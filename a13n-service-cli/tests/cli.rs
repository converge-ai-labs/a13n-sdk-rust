use std::process::{Command, Output};

fn run(argument: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_a13n-service-cli"))
        .arg(argument)
        .output()
        .expect("a13n-service-cli should start")
}

fn normalized_stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone())
        .expect("stdout should be UTF-8")
        .replace("\r\n", "\n")
}

#[test]
fn help_describes_the_cli() {
    let output = run("--help");

    assert!(output.status.success());
    let stdout = normalized_stdout(&output);
    assert!(stdout.contains("Command-line client for a13n Service"));
    assert!(stdout.contains("Usage: a13n-service-cli"));
}

#[test]
fn version_matches_the_package_version() {
    let output = run("--version");

    assert!(output.status.success());
    assert_eq!(
        normalized_stdout(&output),
        format!("a13n-service-cli {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn replacement_requires_an_etag_before_network_access() {
    let output = Command::new(env!("CARGO_BIN_EXE_a13n-service-cli"))
        .args(["labels", "run", "run_test", "--set", "{}"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--if-match"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
}

#[test]
fn every_label_resource_uses_the_sdk_http_contract() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
        time::Duration,
    };
    for (kind, collection) in [
        ("agent", "agents"),
        ("session", "sessions"),
        ("thread", "threads"),
        ("run", "runs"),
        ("skill", "skills"),
        ("environment-template", "environment-templates"),
    ] {
        for replace in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                loop {
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(end) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|length| length.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let request = String::from_utf8(bytes).unwrap();
                assert!(request.starts_with(&format!(
                    "{} /api/v1/workspaces/ws_test/{collection}/resource_test HTTP/1.1",
                    if replace { "PATCH" } else { "GET" }
                )));
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer test-token")
                );
                if replace {
                    assert!(
                        request
                            .to_ascii_lowercase()
                            .contains("if-match: \"original\"")
                    );
                    assert!(request.contains("\"labels\":{\"project\":\"support\"}"));
                }
                use a13n::generated::models;
                let mut body = match kind {
                    "agent" => serde_json::to_value(models::Agent::default()),
                    "session" => serde_json::to_value(models::SessionView::default()),
                    "thread" => serde_json::to_value(models::ThreadView::default()),
                    "run" => serde_json::to_value(models::RunView::default()),
                    "skill" => serde_json::to_value(models::Skill::default()),
                    "environment-template" => serde_json::to_value(models::Template::default()),
                    _ => unreachable!(),
                }
                .unwrap();
                body["labels"] = serde_json::json!({"project": "support"});
                let body = body.to_string();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: \"result\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            });
            let mut command = Command::new(env!("CARGO_BIN_EXE_a13n-service-cli"));
            command.env("A13N_TOKEN", "test-token").args([
                "--base-url",
                &base,
                "labels",
                kind,
                "resource_test",
            ]);
            command.args(["--workspace", "ws_test"]);
            if replace {
                command.args([
                    "--set",
                    r#"{"project":"support"}"#,
                    "--if-match",
                    "\"original\"",
                ]);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(normalized_stdout(&output).contains("support"));
            assert!(String::from_utf8_lossy(&output.stderr).contains("ETag: \"result\""));
            server.join().unwrap();
        }
    }
}

#[test]
fn every_resource_requires_explicit_workspace() {
    for kind in [
        "agent",
        "session",
        "thread",
        "run",
        "skill",
        "environment-template",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_a13n-service-cli"))
            .args(["labels", kind, "resource_test"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("--workspace"));
    }
}

#[test]
fn environments_do_not_offer_nonexistent_labels() {
    let output = Command::new(env!("CARGO_BIN_EXE_a13n-service-cli"))
        .args([
            "labels",
            "environment",
            "env_test",
            "--workspace",
            "ws_test",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid value"));
}

#[test]
fn replacement_rejects_non_string_values_before_network() {
    let output = Command::new(env!("CARGO_BIN_EXE_a13n-service-cli"))
        .args([
            "labels",
            "run",
            "run_test",
            "--workspace",
            "ws_test",
            "--set",
            "{\"x\":1}",
            "--if-match",
            "\"v1\"",
        ])
        .env_remove("A13N_TOKEN")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("string-to-string map"));
}
