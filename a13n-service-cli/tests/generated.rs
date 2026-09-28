use std::{
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    thread,
    time::Duration,
};

type FixtureResponse = (u16, Vec<(&'static str, &'static str)>, Vec<u8>);

fn serve(responses: Vec<FixtureResponse>) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        for (status, headers, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut part = [0; 4096];
            loop {
                let count = stream.read(&mut part).unwrap();
                bytes.extend_from_slice(&part[..count]);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let first = String::from_utf8_lossy(&bytes[..end]);
                    let length = first
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|length| length.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            requests.push(String::from_utf8_lossy(&bytes).into_owned());
            write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n",
                body.len()
            )
            .unwrap();
            for (name, value) in headers {
                write!(stream, "{name}: {value}\r\n").unwrap();
            }
            write!(stream, "\r\n").unwrap();
            stream.write_all(&body).unwrap();
        }
        requests
    });
    (base, handle)
}
fn binary(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_a13n-service-cli"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn paginates_without_losing_page_bodies_or_metadata() {
    let (base, server) = serve(vec![
        (
            200,
            vec![
                ("Content-Type", "application/json"),
                ("X-Request-ID", "request-1"),
            ],
            br#"{"items":[],"next_cursor":"more"}"#.to_vec(),
        ),
        (
            200,
            vec![
                ("Content-Type", "application/json"),
                ("X-Request-ID", "request-2"),
            ],
            br#"{"items":[],"next_cursor":null}"#.to_vec(),
        ),
    ]);
    let output = binary(&[
        "--base-url",
        &base,
        "organizations",
        "list",
        "--limit",
        "1",
        "--all",
        "--include-meta",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(data["pages"].as_array().unwrap().len(), 2);
    assert_eq!(data["pages"][0]["data"]["next_cursor"], "more");
    assert_eq!(data["pages"][0]["request_id"], "request-1");
    assert_eq!(data["pages"][1]["request_id"], "request-2");
    let requests = server.join().unwrap();
    assert!(requests[0].starts_with("GET /api/v1/organizations?limit=1 "));
    assert!(requests[1].starts_with("GET /api/v1/organizations?limit=1&cursor=more "));
}

#[test]
fn bodyless_status_and_redirect_location_remain_visible() {
    let (base, server) = serve(vec![(204, vec![("ETag", "\"gone\"")], Vec::new())]);
    let output = binary(&[
        "--base-url",
        &base,
        "--workspace",
        "ws",
        "grants",
        "delete",
        "grant_x",
        "--include-meta",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(data["status"], 204);
    assert!(data["data"].is_null());
    server.join().unwrap();

    let (base, server) = serve(vec![(
        303,
        vec![("Location", "https://service.example/next")],
        Vec::new(),
    )]);
    let output = binary(&[
        "--base-url",
        &base,
        "connections",
        "callback",
        "get",
        "--state",
        "token",
        "--include-meta",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(data["status"], 303);
    assert_eq!(data["location"], "https://service.example/next");
    assert_eq!(server.join().unwrap().len(), 1); // Never follow the redirect.
}

#[test]
fn binary_download_streams_exact_bytes_and_requires_destination() {
    let body = vec![0x81; 300_000];
    let (base, server) = serve(vec![(200, vec![], body.clone())]);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "a13n-cli-binary-{}-{:?}",
        std::process::id(),
        thread::current().id()
    ));
    let output = binary(&[
        "--base-url",
        &base,
        "--workspace",
        "ws",
        "assets",
        "content",
        "get",
        "--asset",
        "asset_x",
        "--output",
        path.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), body);
    std::fs::remove_file(path).unwrap();
    assert!(server.join().unwrap()[0].starts_with("GET /api/v1/assets/asset_x/content "));
    let output = binary(&[
        "--base-url",
        "http://127.0.0.1:1",
        "--workspace",
        "ws",
        "assets",
        "content",
        "get",
        "--asset",
        "asset_x",
    ]);
    assert_eq!(output.status.code(), Some(2)); // Validate before a GET can happen.
}

#[test]
fn profile_overrides_ambient_target_and_credentials_are_not_printed() {
    let (base, server) = serve(vec![(
        200,
        vec![("Content-Type", "application/json")],
        br#"{"status":"ok"}"#.to_vec(),
    )]);
    let config = std::env::temp_dir().join(format!(
        "a13n-cli-profile-{}-{:?}.json",
        std::process::id(),
        thread::current().id()
    ));
    std::fs::write(&config,serde_json::json!({"profiles":{"prod":{"base_url":base,"workspace":"selected-ws","token_env":"TEST_CLI_TOKEN"}}}).to_string()).unwrap();
    let show = Command::new(env!("CARGO_BIN_EXE_a13n-service-cli"))
        .env("A13N_CLI_CONFIG", &config)
        .env("TEST_CLI_TOKEN", "sensitive-key")
        .env("A13N_BASE_URL", "http://127.0.0.1:1")
        .env("A13N_WORKSPACE", "ambient-ws")
        .args(["--profile", "prod", "config", "show"])
        .output()
        .unwrap();
    assert!(show.status.success());
    let text = String::from_utf8(show.stdout).unwrap();
    assert!(text.contains("selected-ws"));
    assert!(!text.contains("sensitive-key"));
    let output = Command::new(env!("CARGO_BIN_EXE_a13n-service-cli"))
        .env("A13N_CLI_CONFIG", &config)
        .env("TEST_CLI_TOKEN", "sensitive-key")
        .env("A13N_BASE_URL", "http://127.0.0.1:1")
        .env("A13N_WORKSPACE", "ambient-ws")
        .args(["--profile", "prod", "healthz", "get"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(server.join().unwrap()[0].starts_with("GET /healthz "));
    std::fs::remove_file(config).unwrap();
}

#[test]
fn non_generated_dry_run_never_contacts_service() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let output = binary(&[
        "--base-url",
        &base,
        "--workspace",
        "ws",
        "labels",
        "agent",
        "agt_x",
        "--set",
        "{}",
        "--if-match",
        "\"v1\"",
        "--dry-run",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("generated API commands"));
    let output = binary(&[
        "--base-url",
        &base,
        "--workspace",
        "ws",
        "runs",
        "wait",
        "run_x",
        "--dry-run",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(matches!(listener.accept(),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock));
}

#[test]
fn nested_credential_typos_are_rejected_before_http() {
    let body = r#"{"type":"mcp","name":"test","config":{"url":"https://example.net"},"credential":{"token":"fixture-value","tokne":"bad"}}"#;
    let output = binary(&[
        "--base-url",
        "http://127.0.0.1:1",
        "--workspace",
        "ws",
        "workspaces",
        "connections",
        "create",
        "--idempotency-key",
        "key",
        "--body",
        body,
        "--error-format",
        "json",
    ]);
    assert_eq!(output.status.code(), Some(2));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["kind"], "input");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("fixture-value"));
}

#[test]
fn blocked_stdin_obeys_timeout_without_contacting_service() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_a13n-service-cli"))
        .args([
            "--base-url",
            "http://127.0.0.1:1",
            "--workspace",
            "ws",
            "--timeout",
            "1",
            "agents",
            "update",
            "agt_x",
            "--if-match",
            "\"v1\"",
            "--body",
            "@-",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let _stdin = child.stdin.take().unwrap(); // Deliberately leave the pipe open.
    let deadline = std::time::Instant::now() + Duration::from_secs(4);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert_eq!(status.code(), Some(5));
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "blocked stdin ignored --timeout"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(unix)]
#[test]
fn blocked_stdin_obeys_ctrl_c() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_a13n-service-cli"))
        .args([
            "--base-url",
            "http://127.0.0.1:1",
            "--workspace",
            "ws",
            "--timeout",
            "30",
            "agents",
            "update",
            "agt_x",
            "--if-match",
            "\"v1\"",
            "--body",
            "@-",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let _stdin = child.stdin.take().unwrap();
    thread::sleep(Duration::from_millis(300)); // Let Tokio install its signal handler.
    assert!(
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(4);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert_eq!(status.code(), Some(130));
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "blocked stdin ignored Ctrl-C"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn unknown_json_field_and_structured_error_do_not_leak_body() {
    let secret = "DO_NOT_ECHO_SENSITIVE_DATA";
    let output = binary(&[
        "--base-url",
        "http://127.0.0.1:1",
        "--workspace",
        "ws",
        "agents",
        "update",
        "agt_x",
        "--if-match",
        "\"v1\"",
        "--body",
        &format!("{{\"tyop\":\"{secret}\"}}"),
        "--error-format",
        "json",
    ]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains(secret));
    let error: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(error["error"]["kind"], "input");

    let (base, server) = serve(vec![(
        412,
        vec![
            ("Content-Type", "application/json"),
            ("X-Request-ID", "req-safe"),
        ],
        br#"{"error":{"code":"stale_precondition","message":"sensitive-request-data"}}"#.to_vec(),
    )]);
    let output = binary(&[
        "--base-url",
        &base,
        "--workspace",
        "ws",
        "agents",
        "get",
        "agt_x",
        "--error-format",
        "json",
    ]);
    assert_eq!(output.status.code(), Some(4));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("sensitive-request-data"));
    let error: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(error["error"]["status"], 412);
    assert_eq!(error["error"]["code"], "stale_precondition");
    assert_eq!(error["error"]["request_id"], "req-safe");
    server.join().unwrap();
}
