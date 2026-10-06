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
fn ordinary_get_does_not_require_a_binary_destination() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    for arguments in [
        vec!["healthz", "get"],
        vec!["model-providers", "authorization", "get", "--provider", "p"],
    ] {
        let mut args = vec!["--base-url", &base, "--timeout", "1"];
        args.extend(arguments);
        let output = binary(&args);
        assert_eq!(
            output.status.code(),
            Some(5),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("transport failed"));
    }
    let output = binary(&[
        "--dry-run",
        "assets",
        "content",
        "get",
        "--asset",
        "asset_x",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["path"], "/api/v1/assets/asset_x/content");
    assert_eq!(plan["local_only"], true);
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
            "start",
            "agt_x",
            "--idempotency-key",
            "key",
            "--payload",
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
            "start",
            "agt_x",
            "--idempotency-key",
            "key",
            "--payload",
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

#[test]
fn generated_schemas_and_offline_plans_accept_import_and_atomic_resume() {
    let create: serde_json::Value =
        serde_json::from_slice(&binary(&["threads", "create", "--schema"]).stdout).unwrap();
    assert_eq!(
        create["components"]["schemas"]["NewThread"]["properties"]["message_history"]["$ref"],
        "#/components/schemas/MessageHistory"
    );
    let resume: serde_json::Value =
        serde_json::from_slice(&binary(&["runs", "resume", "--schema"]).stdout).unwrap();
    assert_eq!(resume["$ref"], "#/components/schemas/Resume");
    assert!(
        resume["components"]["schemas"]["Resume"]["properties"]
            .get("input")
            .is_some()
    );
    for (arguments, path) in [
        (
            vec![
                "threads",
                "create",
                "--idempotency-key",
                "create-key",
                "--body",
                r#"{"agent_id":"a","payload":{"content":[{"type":"text","text":"Now"}]},"message_history":[{"kind":"request","parts":[{"part_kind":"user-prompt","content":"Before"}]}]}"#,
            ],
            "/api/v1/threads",
        ),
        (
            vec![
                "runs",
                "resume",
                "r",
                "--idempotency-key",
                "resume-key",
                "--body",
                r#"{"approvals":{},"calls":{"c":{"status":"returned","value":{"ok":true}}},"input":{"content":[{"type":"asset","asset_id":"asset_1"}]}}"#,
            ],
            "/api/v1/runs/r/resume",
        ),
    ] {
        let output = binary(
            &["--base-url", "https://service.invalid", "--dry-run"]
                .into_iter()
                .chain(arguments)
                .collect::<Vec<_>>(),
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(plan["path"], path);
        assert_eq!(plan["body"], "[REDACTED]");
        assert_eq!(plan["server_validated"], false);
    }
}

#[test]
fn generated_dispatch_preserves_import_objects_and_resume_input_on_the_wire() {
    for (commands, expected_path, field, expected) in [
        (
            vec![
                "threads",
                "create",
                "--idempotency-key",
                "create-key",
                "--body",
                r#"{"agent_id":"a","payload":{"content":[{"type":"text","text":"Now"}]},"message_history":[{"kind":"request","parts":[{"part_kind":"user-prompt","content":"Before"}],"metadata":{"origin":"external"}}]}"#,
            ],
            "/api/v1/threads",
            "message_history",
            "external",
        ),
        (
            vec![
                "runs",
                "resume",
                "r",
                "--idempotency-key",
                "resume-key",
                "--body",
                r#"{"approvals":{},"calls":{"c":{"status":"returned","value":{"ok":true}}},"input":{"content":[{"type":"asset","asset_id":"asset_1"}]}}"#,
            ],
            "/api/v1/runs/r/resume",
            "input",
            "asset_1",
        ),
    ] {
        // The local origin rejects the request after capture; no live Service or model is involved.
        let (base, origin) = serve(vec![(
            400,
            vec![("Content-Type", "application/json")],
            br#"{"error":{"code":"invalid_argument","message":"local fixture"}}"#.to_vec(),
        )]);
        let output = binary(
            &["--base-url", &base]
                .into_iter()
                .chain(commands)
                .collect::<Vec<_>>(),
        );
        assert!(!output.status.success());
        let received = origin.join().unwrap();
        assert_eq!(received.len(), 1);
        assert!(received[0].starts_with(&format!("POST {expected_path} ")));
        let body: serde_json::Value =
            serde_json::from_str(received[0].split_once("\r\n\r\n").unwrap().1).unwrap();
        if field == "message_history" {
            assert_eq!(body[field][0]["metadata"]["origin"], expected);
        } else {
            assert_eq!(body[field]["content"][0]["asset_id"], expected);
            assert_eq!(body["calls"]["c"]["value"]["ok"], true);
        }
    }
}

#[test]
fn thread_events_require_paired_coverage_and_preserve_gap_targets_and_reconnect_baseline() {
    for argument in ["--run", "--position"] {
        let output = binary(&["threads", "events", "--thread", "t", argument, "1-2"]);
        assert_eq!(output.status.code(), Some(2));
    }
    let output = binary(&[
        "threads",
        "events",
        "--thread",
        "t",
        "--run",
        "r",
        "--position",
        "01-2",
    ]);
    assert_eq!(output.status.code(), Some(2));
    let first = b"event: boundary\nid: 100-2\ndata: {\"run_id\":\"r\",\"attempt\":1,\"sequence\":2}\n\nevent: delta\nid: 100-3\ndata: {\"run_id\":\"r\",\"attempt\":1,\"sequence\":3,\"event\":{},\"item\":null}\n\n".to_vec();
    let second = b"event: gap\ndata: {\"run_id\":\"r\",\"position\":\"1-5\"}\n\nevent: gap\ndata: {\"run_id\":\"r\"}\n\nevent: delta\nid: 100-4\ndata: {\"run_id\":\"r\",\"attempt\":1,\"sequence\":4,\"event\":{},\"item\":null}\n\n".to_vec();
    let (base, origin) = serve(vec![
        (200, vec![("Content-Type", "text/event-stream")], first),
        (200, vec![("Content-Type", "text/event-stream")], second),
        (
            404,
            vec![("Content-Type", "application/json")],
            br#"{"error":{"code":"not_found","message":"end"}}"#.to_vec(),
        ),
    ]);
    let output = binary(&[
        "--base-url",
        &base,
        "threads",
        "events",
        "--thread",
        "t",
        "--run",
        "r",
        "--position",
        "1-2",
        "--after",
        "100-2",
        "--max-reconnects",
        "1",
    ]);
    assert!(!output.status.success()); // The origin deliberately ends retries with a terminal refusal.
    let frames: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(frames[0]["type"], "boundary");
    assert_eq!(frames[1]["applied_position"], "1-2");
    assert_eq!(frames[2]["position"], "1-5");
    assert!(frames[3]["position"].is_null());
    assert_eq!(frames[4]["applied_position"], "1-3");
    let requests = origin.join().unwrap();
    assert!(requests[0].starts_with("GET /api/v1/threads/t/stream?run=r&position=1-2 "));
    assert!(requests[1].starts_with("GET /api/v1/threads/t/stream?run=r&position=1-3 "));
    assert!(
        requests[1]
            .to_ascii_lowercase()
            .contains("last-event-id: 100-3")
    );
    assert!(requests[2].starts_with("GET /api/v1/threads/t/stream?run=r&position=1-3 "));
    assert!(
        requests[2]
            .to_ascii_lowercase()
            .contains("last-event-id: 100-4")
    );
}

#[test]
fn generated_stream_dispatch_forwards_both_queries_and_hint_without_a_reducer() {
    let file = std::env::temp_dir().join(format!(
        "a13n-stream-{}-{:?}",
        std::process::id(),
        thread::current().id()
    ));
    let (base, origin) = serve(vec![(
        200,
        vec![("Content-Type", "text/event-stream")],
        b": heartbeat\n\n".to_vec(),
    )]);
    let output = binary(&[
        "--base-url",
        &base,
        "threads",
        "stream",
        "get",
        "--thread",
        "t",
        "--run",
        "r",
        "--position",
        "1-2",
        "--last-event-id",
        "100-2",
        "--output",
        file.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = origin.join().unwrap();
    assert!(requests[0].starts_with("GET /api/v1/threads/t/stream?run=r&position=1-2 "));
    assert!(
        requests[0]
            .to_ascii_lowercase()
            .contains("last-event-id: 100-2")
    );
    assert_eq!(std::fs::read(&file).unwrap(), b": heartbeat\n\n");
    std::fs::remove_file(file).unwrap();
}

#[test]
fn schemas_and_dispatch_preserve_native_settings_upload_ids_and_eight_character_passwords() {
    let schema: serde_json::Value =
        serde_json::from_slice(&binary(&["models", "create", "--schema"]).stdout).unwrap();
    let schemas = &schema["components"]["schemas"];
    assert_eq!(
        schemas["AssetCreate"]["properties"]["upload_id"]["pattern"],
        "^upl_[a-f0-9]{32}$"
    );
    assert_eq!(
        schemas["UploadSource"]["properties"]["upload_id"]["pattern"],
        "^upl_[a-f0-9]{32}$"
    );
    for name in ["BootstrapInput", "PasswordChange", "PasswordResetConfirm"] {
        assert_eq!(schemas[name]["properties"]["password"]["minLength"], 8);
    }
    assert_eq!(
        schemas["ModelConfig-Input"]["properties"]["settings"]["additionalProperties"]["$ref"],
        "#/components/schemas/JsonValue"
    );
    for (commands, body) in [
        (
            vec!["models", "create"],
            r#"{"name":"model","provider_id":"p","config":{"model_api":"provider:model","model_name":"model","settings":{"custom":{"choices":[true,null,2]},"timeout":2.5}}}"#,
        ),
        (
            vec!["assets", "create"],
            r#"{"name":"asset","upload_id":"upl_0123456789abcdef0123456789abcdef"}"#,
        ),
        (
            vec!["auth", "bootstrap"],
            r#"{"email":"test@example.org","password":"12345678"}"#,
        ),
        (
            vec!["users", "me", "password"],
            r#"{"current_password":"old","password":"12345678"}"#,
        ),
        (
            vec!["auth", "password-reset", "confirm"],
            r#"{"token":"token","password":"12345678"}"#,
        ),
    ] {
        let (base, origin) = serve(vec![(
            400,
            vec![("Content-Type", "application/json")],
            br#"{"error":{"code":"invalid_argument","message":"capture"}}"#.to_vec(),
        )]);
        let output = binary(
            &["--base-url", &base]
                .into_iter()
                .chain(commands)
                .chain(["--body", body])
                .collect::<Vec<_>>(),
        );
        assert!(!output.status.success());
        let requests = origin.join().unwrap();
        let sent: serde_json::Value =
            serde_json::from_str(requests[0].split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(
            sent,
            serde_json::from_str::<serde_json::Value>(body).unwrap()
        );
    }
}

#[test]
fn authored_payload_and_options_preserve_native_configuration_on_the_wire() {
    let payload = serde_json::json!({"content":[{"type":"text","text":""},{"type":"url","url":"https://media.example/image.png"},{"type":"asset","asset_id":"asset_native"}]});
    let file = std::env::temp_dir().join(format!(
        "a13n-native-{}-{:?}.json",
        std::process::id(),
        thread::current().id()
    ));
    std::fs::write(&file, payload.to_string()).unwrap();
    for config in [
        None,
        Some(serde_json::Value::Null),
        Some(serde_json::json!({})),
        Some(serde_json::json!({"allowed_hosts":null})),
        Some(serde_json::json!({"allowed_hosts":[]})),
        Some(
            serde_json::json!({"allowed_hosts":["media.example"],"extensions":{"example.policy":{"enabled":false,"zero":0,"empty":[],"nested":{"value":null}}}}),
        ),
    ] {
        let mut options = serde_json::json!({});
        if let Some(config) = config {
            options["configuration"] = config;
        }
        for action in ["start", "send", "generated"] {
            let (base, origin) = serve(vec![(409, vec![("Content-Type", "application/json")], br#"{"error":{"code":"conflict","message":"capture","details":{"reason":"run_configuration_immutable"}}}"#.to_vec())]);
            let output = if action == "generated" {
                let body = serde_json::json!({"agent_id":"a","payload":payload,"options":options})
                    .to_string();
                binary(&[
                    "--base-url",
                    &base,
                    "threads",
                    "create",
                    "--idempotency-key",
                    "native-key",
                    "--body",
                    &body,
                ])
            } else {
                let raw_options = options.to_string();
                let raw_payload = format!("@{}", file.display());
                let mut args = vec![
                    "--base-url",
                    &base,
                    "agents",
                    action,
                    "a",
                    "--payload",
                    &raw_payload,
                    "--options",
                    &raw_options,
                    "--idempotency-key",
                    "native-key",
                ];
                if action == "send" {
                    args.extend(["--thread", "t"]);
                }
                binary(&args)
            };
            assert_eq!(output.status.code(), Some(4));
            let requests = origin.join().unwrap();
            assert_eq!(requests.len(), 1); // Service conflict is never silently retried.
            let sent: serde_json::Value =
                serde_json::from_str(requests[0].split_once("\r\n\r\n").unwrap().1).unwrap();
            assert_eq!(sent["payload"], payload);
            assert_eq!(sent["options"], options);
        }
    }
    std::fs::remove_file(file).unwrap();
    let output = binary(&[
        "agents",
        "start",
        "a",
        "--text",
        "",
        "--payload",
        "{\"content\":[]}",
        "--idempotency-key",
        "k",
    ]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn authored_json_stdin_and_empty_text_keep_existing_input_ownership() {
    for (flag, value) in [
        ("--payload", r#"{"content":[]}"#),
        (
            "--options",
            r#"{"configuration":{"extensions":{"example":{"enabled":false}}}}"#,
        ),
    ] {
        let (base, origin) = serve(vec![(
            400,
            vec![("Content-Type", "application/json")],
            br#"{"error":{"code":"invalid_argument","message":"capture"}}"#.to_vec(),
        )]);
        let mut args = vec![
            "--base-url",
            &base,
            "agents",
            "start",
            "a",
            flag,
            "@-",
            "--idempotency-key",
            "k",
        ];
        if flag == "--options" {
            args.extend(["--text", ""]);
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_a13n-service-cli"))
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(value.as_bytes())
            .unwrap();
        assert_eq!(child.wait_with_output().unwrap().status.code(), Some(2));
        let requests = origin.join().unwrap();
        let sent: serde_json::Value =
            serde_json::from_str(requests[0].split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(
            sent[if flag == "--payload" {
                "payload"
            } else {
                "options"
            }],
            serde_json::from_str::<serde_json::Value>(value).unwrap()
        );
        if flag == "--options" {
            assert_eq!(sent["payload"]["content"][0]["text"], "");
        }
    }
    let output = binary(&[
        "--base-url",
        "http://127.0.0.1:1",
        "agents",
        "start",
        "a",
        "--text",
        "x",
        "--options",
        r#"{"configuration":{"allowed_host":[]}}"#,
        "--idempotency-key",
        "k",
    ]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn native_agui_child_media_and_unknown_custom_values_are_printed_without_reduction() {
    let events = vec![
        serde_json::json!({"type":"RUN_FINISHED","threadId":"t","runId":"child","subagentRunId":"child"}),
        serde_json::json!({"type":"TOOL_CALL_RESULT","messageId":"same","toolCallId":"call","role":"tool","subagentRunId":"child","content":[{"type":"text","text":"tool"},{"type":"binary","mimeType":"video/mp4","url":"https://media.example/clip.mp4","source":{"kind":"file","path":"clip.mp4"}},{"type":"binary","mimeType":"image/png","data":null,"metadata":{"payload_omitted":true}}]}),
        serde_json::json!({"type":"CUSTOM","name":"future.native","value":null}),
    ];
    let mut body = String::new();
    for (index, event) in events.iter().enumerate() {
        body.push_str(&format!("event: delta\nid: 100-{}\ndata: {}\n\n",index+1,serde_json::json!({"run_id":"root","attempt":1,"sequence":index+1,"event":event,"item":null})));
    }
    let (base, origin) = serve(vec![(
        200,
        vec![("Content-Type", "text/event-stream")],
        body.into_bytes(),
    )]);
    let output = binary(&["--base-url", &base, "threads", "events", "--thread", "t"]);
    let frames: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(frames.len(), events.len());
    for (frame, event) in frames.iter().zip(events) {
        assert_eq!(frame["run_id"], "root");
        assert_eq!(frame["event"], event);
    }
    assert_eq!(origin.join().unwrap().len(), 1);
}

#[test]
fn five_oauth_commands_use_generated_dispatch_and_preserve_nullable_disconnection() {
    for (commands, method, suffix, body, response) in [
        (
            vec!["model-providers", "authorization", "get", "--provider", "p"],
            "GET",
            "/authorization",
            None,
            serde_json::json!({"provider_id":"p","state":"disconnected"}),
        ),
        (
            vec!["model-providers", "authorize", "p"],
            "POST",
            "/authorize",
            Some(r#"{"new_registration":false}"#),
            serde_json::json!({"attempt_id":"attempt","method":"manual_callback","authorization_url":"https://provider.example/authorize","expires_at":"2026-10-01T12:00:00Z"}),
        ),
        (
            vec!["model-providers", "authorization", "callback", "p"],
            "POST",
            "/authorization/callback",
            Some(
                r#"{"attempt_id":"attempt","callback_url":"https://callback.example/?code=private"}"#,
            ),
            serde_json::json!({"provider_id":"p","state":"connected"}),
        ),
        (
            vec![
                "model-providers",
                "authorization",
                "delete",
                "--provider",
                "p",
            ],
            "DELETE",
            "/authorization",
            None,
            serde_json::json!({"local_tokens_cleared":true,"revocation_confirmed":null}),
        ),
        (
            vec!["model-providers", "models", "get", "--provider", "p"],
            "GET",
            "/models",
            None,
            serde_json::json!([{"slug":"native","display_name":"Native"}]),
        ),
    ] {
        let (base, origin) = serve(vec![(
            200,
            vec![("Content-Type", "application/json")],
            response.to_string().into_bytes(),
        )]);
        let mut args = vec!["--base-url", &base, "--include-meta"];
        args.extend(commands);
        if let Some(body) = body {
            args.extend(["--body", body]);
        }
        let output = binary(&args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let data: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(data["status"], 200);
        // Deserialize/serialize may add optional defaults; native response evidence is retained.
        if suffix == "/models" {
            assert_eq!(data["data"][0]["slug"], "native");
        }
        if method == "DELETE" {
            assert!(data["data"]["revocation_confirmed"].is_null());
        }
        let requests = origin.join().unwrap();
        assert!(requests[0].starts_with(&format!("{method} /api/v1/model-providers/p{suffix} ")));
        if let Some(body) = body {
            let sent: serde_json::Value =
                serde_json::from_str(requests[0].split_once("\r\n\r\n").unwrap().1).unwrap();
            assert_eq!(
                sent,
                serde_json::from_str::<serde_json::Value>(body).unwrap()
            );
        }
    }
}

#[test]
fn ordinal_items_flags_dispatch_json_not_binary_and_preserve_window_metadata() {
    use a13n::generated::models::{DisplayContinuation, Item, RunItems, StreamPosition};
    let mut snapshot = RunItems::default();
    snapshot.run.id = "r".into();
    snapshot.run.display_position = Some(Some("1-9".into()));
    snapshot.complete = true;
    snapshot.baseline = true;
    snapshot.position = Some("1-9".into());
    snapshot.continuation = Some(Some(Box::new(DisplayContinuation::new(
        10,
        StreamPosition::new(1, 9),
        "r".into(),
    ))));
    snapshot.resume_after = Some(Some("100-9".into()));
    snapshot.items = (6..=9)
        .map(|ordinal| Item {
            ordinal,
            id: format!("i{ordinal}"),
            ..Default::default()
        })
        .collect();
    let expected = serde_json::to_value(&snapshot).unwrap();
    let (base, origin) = serve(vec![(
        200,
        vec![
            ("Content-Type", "application/json"),
            ("X-Request-ID", "items-request"),
        ],
        serde_json::to_vec(&snapshot).unwrap(),
    )]);
    let output = binary(&[
        "--base-url",
        &base,
        "runs",
        "items",
        "get",
        "--run",
        "r",
        "--limit",
        "2",
        "--include-meta",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["data"], expected);
    assert_eq!(result["request_id"], "items-request");
    assert!(origin.join().unwrap()[0].starts_with("GET /api/v1/runs/r/items?limit=2 "));
    snapshot.baseline = false;
    snapshot.position = None;
    snapshot.continuation = Some(None);
    snapshot.resume_after = Some(None);
    snapshot.items.truncate(2);
    for (flag, value) in [("--before", "8"), ("--after", "0")] {
        let (base, origin) = serve(vec![(
            200,
            vec![("Content-Type", "application/json")],
            serde_json::to_vec(&snapshot).unwrap(),
        )]);
        let output = binary(&[
            "--base-url",
            &base,
            "runs",
            "items",
            "get",
            "--run",
            "r",
            flag,
            value,
            "--limit",
            "2",
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result, serde_json::to_value(&snapshot).unwrap());
        assert!(origin.join().unwrap()[0].starts_with(&format!(
            "GET /api/v1/runs/r/items?{}={value}&limit=2 ",
            &flag[2..]
        )));
    }
    let (base, origin) = serve(vec![(
        400,
        vec![("Content-Type", "application/json")],
        br#"{"error":{"code":"invalid_argument","message":"mutually exclusive"}}"#.to_vec(),
    )]);
    let output = binary(&[
        "--base-url",
        &base,
        "runs",
        "items",
        "get",
        "--run",
        "r",
        "--before",
        "1",
        "--after",
        "0",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(origin.join().unwrap()[0].starts_with("GET /api/v1/runs/r/items?before=1&after=0 "));
}

#[test]
fn events_jsonl_preserves_optional_null_and_populated_item_metadata() {
    use serde_json::json;
    for item in [
        json!({"id":"i","kind":"observation","state":"failed"}),
        json!({"id":"i","kind":"observation","state":"failed","ordinal":null,"response_group":null,"failure":null}),
        json!({"id":"i","kind":"observation","state":"failed","ordinal":42,"response_group":"child","failure":{"custom":[false,null,[]]}}),
    ] {
        let event = json!({"type":"CUSTOM","subagentRunId":"child","value":{"unknown":null}});
        let body = format!(
            "event: delta\nid: 100-1\ndata: {}\n\n",
            json!({"run_id":"r","attempt":1,"sequence":1,"event":event,"item":item})
        );
        let (base, origin) = serve(vec![(
            200,
            vec![("Content-Type", "text/event-stream")],
            body.into_bytes(),
        )]);
        let output = binary(&["--base-url", &base, "threads", "events", "--thread", "t"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let frame: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(frame["item"], item);
        assert_eq!(frame["event"], event);
        assert!(frame["applied_cursor"].is_null());
        assert_eq!(origin.join().unwrap().len(), 1);
    }
}
