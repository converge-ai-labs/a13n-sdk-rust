//! Ordinary workflows consume ONLY root public SDK exports from the installed crate.
//! Fixture JSON is supplied by the separate advanced protocol acceptance module.
use super::{Result, ensure};
use a13n::{
    Answer, Client, Error, ForkOptions, Input, InputPart, ObservationOptions, Overrides,
    PreviewInterruptedKind, ReceiptEvidence, ResumeOptions, RunConfiguration, RunOptions,
    RunStatus, Secret, StartOptions,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct AbortOnDrop(tokio::task::AbortHandle);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
fn delta(sequence: usize, event: Value) -> String {
    format!(
        "event: delta\nid: {sequence}-0\ndata: {}\n\n",
        json!({"run_id":"r2","attempt":1,"sequence":sequence,"event":event,"item":null})
    )
}
pub async fn offline(receipt: Value, entry: Value, run: Value) -> Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let mut successor = run.clone();
    successor["id"] = json!("r2");
    successor["status"] = json!("running");
    let mut completed = successor.clone();
    completed["status"] = json!("completed");
    completed["output"] = json!("");
    let stream = [
        delta(1, json!({"type":"TEXT_MESSAGE_START","messageId":"message","role":"assistant"})),
        delta(2, json!({"type":"TEXT_MESSAGE_CONTENT","messageId":"message","delta":"hello"})),
        delta(3, json!({"type":"CUSTOM","name":"a13n.stream.fragment","subagentRunId":"child","value":{"id":"opaque-child","index":0,"count":2,"data":"{}"}})),
        delta(4, json!({"type":"CUSTOM","name":"a13n.stream.fragment","value":{"id":"opaque-root","index":0,"count":2,"data":"{}"}})),
        delta(5, json!({"type":"CUSTOM","name":"unrelated","value":null})),
    ].concat();
    // A bounded script rejects extra requests (including Items or a mutation retry).
    let replies = vec![
        ("POST /api/v1/threads ", receipt.to_string()),
        ("GET /api/v1/threads/t/inbox/e ", entry.to_string()),
        ("GET /api/v1/runs/r ", run.to_string()),
        ("POST /api/v1/threads ", receipt.to_string()),
        ("POST /api/v1/threads/t/inbox ", receipt.to_string()),
        ("GET /api/v1/threads/t/inbox/e ", entry.to_string()),
        ("GET /api/v1/runs/r ", run.to_string()),
        ("POST /api/v1/runs/r/resume ", successor.to_string()),
        ("GET /api/v1/threads/t/stream ", stream),
        ("GET /api/v1/runs/r2 ", completed.to_string()),
        ("GET /api/v1/runs/r ", run.to_string()),
        ("POST /api/v1/runs/r/fork ", receipt.to_string()),
        (
            "POST /api/v1/threads ",
            "<html>gateway timeout</html>".into(),
        ),
        ("GET /api/v1/runs/r ", run.to_string()),
        ("GET /api/v1/runs/r ", run.to_string()),
    ];
    let server = tokio::spawn(async move {
        let mut chosen_keys = Vec::new();
        for (index, (expected, body)) in replies.into_iter().enumerate() {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let header_end = loop {
                let mut bytes = [0; 4096];
                let count = socket.read(&mut bytes).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&bytes[..count]);
                if let Some(end) = request.windows(4).position(|p| p == b"\r\n\r\n") {
                    break end + 4;
                }
                assert!(request.len() < 65_536);
            };
            let headers = String::from_utf8(request[..header_end].to_vec()).unwrap();
            assert!(headers.starts_with(expected), "request {index}: {headers}");
            let lower = headers.to_ascii_lowercase();
            assert!(lower.contains("authorization: bearer offline-token\r\n"));
            let length = lower
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .map(|value| value.trim().parse::<usize>().unwrap())
                .unwrap_or(0);
            while request.len() - header_end < length {
                let mut bytes = [0; 4096];
                let count = socket.read(&mut bytes).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&bytes[..count]);
            }
            if expected.starts_with("POST") {
                let key = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':').and_then(|(name, value)| {
                            name.eq_ignore_ascii_case("idempotency-key")
                                .then(|| value.trim().to_owned())
                        })
                    })
                    .unwrap();
                assert!(!key.is_empty());
                chosen_keys.push(key.clone());
                let value: Value =
                    serde_json::from_slice(&request[header_end..header_end + length]).unwrap();
                match index {
                    0 => {
                        assert_eq!(key.len(), 32);
                        assert_eq!(
                            value["payload"]["content"][0],
                            json!({"type":"text","text":"first reply"})
                        );
                    }
                    3 => {
                        assert_eq!(key, "persisted-key");
                        assert_eq!(
                            value["payload"]["content"],
                            json!([
                                {"type":"text","text":""}, {"type":"json","value":null},
                                {"type":"url","url":"https://example.test/image"},
                                {"type":"asset","asset_id":"ast_1"}
                            ])
                        );
                        assert_eq!(
                            value["options"]["configuration"]["extensions"]["test"],
                            json!({"flag":false,"zero":0,"empty":[]})
                        );
                        assert_eq!(
                            value["options"]["overrides"]["model_settings"]["nested"],
                            json!({"value":null})
                        );
                        assert_eq!(value["options"]["overrides"]["instructions"], "");
                        assert!(value["options"]["overrides"]["model"].is_null());
                    }
                    7 => {
                        assert_eq!(
                            value["approvals"],
                            json!({"approval":{"action":"deny","reason":null}})
                        );
                        assert_eq!(
                            value["calls"],
                            json!({"call":{"status":"returned","value":null}})
                        );
                        assert_eq!(value["input"]["content"][0]["text"], "resume context");
                    }
                    11 => assert_eq!(value["agent_id"], "agent"),
                    _ => {}
                }
            }
            let (status, kind) = if index == 12 {
                (504, "text/html")
            } else if index == 8 {
                (200, "text/event-stream")
            } else {
                (200, "application/json")
            };
            socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
        chosen_keys
    });
    let _cleanup = AbortOnDrop(server.abort_handle());
    let client = Client::new(&base, Secret::new("offline-token"))?;
    let agent = client.agent("agent");
    let mut first = agent.start("first reply").await?;
    let first_key = first
        .identity
        .idempotency_key
        .clone()
        .ok_or("Missing automatic key")?;
    ensure(
        first.run.is_none(),
        "Receipt must not make provisional Run authoritative",
    )?;
    let outcome = first.result().await?;
    ensure(
        outcome.status == RunStatus::Completed
            && outcome.output == run["output"]
            && outcome.text.is_none(),
        "Structured full output without Items",
    )?;
    ensure(
        outcome.decode_output::<Value>()? == run["output"],
        "Typed structured decoding",
    )?;
    let persisted = serde_json::to_string(
        &json!({"thread_id":first.identity.thread_id,"entry_id":first.identity.entry_id,"key":first_key}),
    )?;
    let saved: Value = serde_json::from_str(&persisted)?;
    let mut configured = agent
        .start_with(
            Input::parts([
                InputPart::text(""),
                InputPart::json(Value::Null),
                InputPart::url("https://example.test/image"),
                InputPart::asset("ast_1"),
            ]),
            StartOptions {
                idempotency_key: Some("persisted-key".into()),
                run_options: Some(
                    RunOptions::new()
                        .configuration(Some(
                            RunConfiguration::new()
                                .allowed_hosts(None)
                                .extensions(HashMap::from([(
                                    "test".into(),
                                    json!({"flag":false,"zero":0,"empty":[]}),
                                )])),
                        ))
                        .overrides(Some(
                            Overrides::new()
                                .model(None)
                                .instructions(Some(String::new()))
                                .model_settings(Some(HashMap::from([(
                                    "nested".into(),
                                    json!({"value":null}),
                                )]))),
                        )),
                ),
                ..Default::default()
            },
        )
        .await?;
    configured.close();
    let mut followup = agent.send("t", "follow-up").await?;
    let followup_key = followup.identity.idempotency_key.clone().unwrap();
    ensure(
        first_key != followup_key,
        "Each logical mutation chooses a distinct key",
    )?;
    followup.close();
    let mut recovered = client
        .entry(
            saved["thread_id"].as_str().unwrap(),
            saved["entry_id"].as_str().unwrap(),
        )
        .observe(ObservationOptions::default())?;
    ensure(
        matches!(recovered.evidence, ReceiptEvidence::RecoveredIdentity),
        "Recovery must not fabricate HTTP evidence",
    )?;
    ensure(
        recovered.result().await?.run_id == "r",
        "Exact Entry incorporation recovery",
    )?;
    let mut resumed = outcome
        .run
        .resume_with(
            HashMap::from([
                ("approval".into(), Answer::deny_with(None)),
                ("call".into(), Answer::returned(Value::Null)),
            ]),
            ResumeOptions {
                input: Some(Some("resume context".into())),
                ..Default::default()
            },
        )
        .await?;
    ensure(
        resumed.identity.entry_id.is_none()
            && resumed.identity.run_id.as_deref() == Some("r2")
            && resumed.evidence.resume().is_some(),
        "Exact successor Interaction evidence",
    )?;
    ensure(
        resumed.next_text().await?.as_deref() == Some("hello"),
        "Root-only preview excludes child framing",
    )?;
    ensure(
        matches!(resumed.next_text().await, Err(Error::PreviewInterrupted(ref e)) if e.kind == PreviewInterruptedKind::Fragment),
        "Canonical root fragment unsupported framing",
    )?;
    ensure(
        matches!(resumed.next_text().await, Err(Error::PreviewInterrupted(_))),
        "Preview interruption is sticky",
    )?;
    ensure(
        resumed.next().await?.is_some(),
        "Sequential raw access after interruption",
    )?;
    let resumed_output = resumed.result().await?;
    ensure(
        resumed_output.text.as_deref() == Some("") && resumed_output.output == json!(""),
        "Interrupted preview preserves authoritative empty-string outcome",
    )?;
    let mut exact = client.run("r").observe(ObservationOptions::default())?;
    ensure(
        exact.result().await?.run_id == "r",
        "Exact Run uses same Interaction core",
    )?;
    let mut fork = outcome
        .run
        .fork("agent", "fork", ForkOptions::default())
        .await?;
    fork.close();
    let unknown_key = match agent.start("gateway unknown").await {
        Err(Error::UnknownOutcome(e)) => {
            ensure(
                e.http_status == Some(504) && matches!(e.cause, Error::Api(_)),
                "Unknown gateway cause/status evidence",
            )?;
            e.idempotency_key
        }
        _ => return Err("Gateway response falsely classified as known rejection".into()),
    };
    let owned = client.run("r");
    let temporary = client.clone();
    drop(temporary);
    ensure(
        tokio::spawn(async move { owned.result(ObservationOptions::default()).await })
            .await??
            .run_id
            == "r",
        "Owned task-safe handles retain shared Client",
    )?;
    let native = client.run("r").get().await?;
    ensure(
        native.data.id == "r" && native.data.status == RunStatus::Completed,
        "Canonical native api and authored workflow share one Client/auth/lifetime",
    )?;
    let keys = server.await?;
    ensure(
        keys[0] == first_key && keys[2] == followup_key && keys[5] == unknown_key,
        "Keys exposed on identity and unknown-outcome error match actual requests",
    )?;
    let api_run = client.api().runs().at("r");
    client.clone().close();
    ensure(
        matches!(api_run.get(Default::default()).await, Err(Error::Closed)),
        "Native api handles share explicit Client close",
    )?;
    ensure(
        matches!(
            client.run("r").result(ObservationOptions::default()).await,
            Err(Error::Closed)
        ),
        "Explicit close is shared by all clones",
    )?;
    println!(
        "Installed crate canonical api plus root-only workflows: same Client/auth/shared close, minimal start/send, multipart/config/overrides, full structured output, saved identity recovery, one-map resume, finite text/raw/result, fork, optional keys, gateway unknown outcome and task-safe shared close passed"
    );
    Ok(())
}
