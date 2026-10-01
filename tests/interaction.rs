mod common;
use a13n::{Error, StartOptions, generated::models, streaming::ThreadFrame};
use common::{Reply, client, sample, server};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

fn receipt() -> Value {
    let mut value = sample("Submitted");
    value["thread"]["id"] = json!("t");
    value["thread"]["workspace_id"] = json!("w");
    value["entry"]["id"] = json!("e");
    value["entry"]["thread_id"] = json!("t");
    value["run"] = Value::Null;
    value
}
fn entry(status: &str, assigned: Option<&str>) -> Value {
    let mut value = sample("EntryView");
    value["id"] = json!("e");
    value["thread_id"] = json!("t");
    value["status"] = json!(status);
    value["assigned_run_id"] = json!(assigned);
    value
}
fn run(status: &str) -> Value {
    let mut value = sample("RunView");
    value["id"] = json!("r");
    value["thread_id"] = json!("t");
    value["status"] = json!(status);
    value
}

#[tokio::test]
async fn result_only_waits_for_consumed_entry_not_provisional_assignment() {
    let polls = Arc::new(AtomicUsize::new(0));
    let calls = polls.clone();
    let mut fixture = server(move |request| {
        if request.method == "POST" {
            return Reply::json(200, receipt());
        }
        if request.target.contains("/inbox/e") {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            return Reply::json(
                200,
                match n {
                    0 => entry("pending", None),
                    1 => entry("assigned", Some("wrong_run")),
                    _ => entry("consumed", Some("r")),
                },
            );
        }
        assert!(request.target.ends_with("/runs/r"), "{}", request.target);
        Reply::json(200, run("waiting"))
    })
    .await;
    let sdk = client(&fixture);
    let mut interaction = sdk.agent("agent_id").start("hello", "once").await.unwrap();
    assert_eq!(interaction.receipt.data.thread.id, "t");
    assert_eq!(interaction.receipt.data.entry.id, "e");
    let result = tokio::time::timeout(Duration::from_secs(3), interaction.result())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*result.status(), models::RunStatus::Waiting);
    assert!(interaction.next().await.unwrap().is_none());
    assert_eq!(polls.load(Ordering::SeqCst), 3);
    let mut posts = 0;
    while let Ok(request) = fixture.requests.try_recv() {
        if request.method == "POST" {
            posts += 1;
            assert_eq!(request.json()["agent_id"], "agent_id");
        }
    }
    assert_eq!(posts, 1);
}

#[tokio::test]
async fn start_imports_native_history_once_and_thread_readback_preserves_json() {
    let history = json!([
        {"kind":"request", "parts":[{"part_kind":"user-prompt", "content":"Earlier question"}],
         "metadata":{"external":"value", "count": 3}},
        {"kind":"response", "parts":[{"part_kind":"text", "content":"Earlier answer"}],
         "usage":{"input_tokens": 5}}
    ]);
    let mut accepted = receipt();
    accepted["thread"]["message_history"] = history.clone();
    let mut thread = sample("ThreadView");
    thread["id"] = json!("t");
    thread["message_history"] = history.clone();
    let mut fixture = server(move |request| {
        if request.method == "POST" {
            Reply::json(201, accepted.clone())
        } else {
            assert!(request.target.ends_with("/threads/t"));
            Reply::json(200, thread.clone())
        }
    })
    .await;
    let sdk = client(&fixture);
    let imported = serde_json::from_value(history.clone()).unwrap();
    let interaction = sdk
        .agent("agent_id")
        .start_with(
            "New question",
            "import-once",
            StartOptions {
                message_history: Some(imported),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&interaction.receipt.data.thread.message_history).unwrap(),
        history
    );
    let readback = interaction
        .thread
        .resource()
        .get(Default::default())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&readback.data.message_history).unwrap(),
        history
    );
    sdk.agent("agent_id")
        .send("t", "Follow up", "new-message")
        .await
        .unwrap();
    let first = fixture.requests.recv().await.unwrap();
    assert_eq!(first.json()["message_history"], history);
    assert_eq!(
        first.json()["payload"]["content"][0]["text"],
        "New question"
    );
    assert!(
        !fixture
            .requests
            .recv()
            .await
            .unwrap()
            .target
            .contains("/inbox")
    );
    let followup = fixture.requests.recv().await.unwrap();
    assert!(followup.target.ends_with("/threads/t/inbox"));
    assert!(followup.json().get("message_history").is_none());
    assert_eq!(
        followup.json()["payload"]["content"][0]["text"],
        "Follow up"
    );
}

#[tokio::test]
async fn send_rejects_receipt_for_a_different_thread() {
    let fixture = server(|_| Reply::json(200, receipt())).await;
    let sdk = client(&fixture);
    assert!(
        matches!(sdk.agent("agent_id").send("another", "hello", "once").await,
        Err(Error::Protocol(error)) if error.kind == a13n::ProtocolKind::InvalidReceipt)
    );
}

#[tokio::test]
async fn authored_run_resume_binds_distinct_successor_and_interrupts_only_explicitly() {
    let mut fixture = server(|request| {
        if request.method == "POST" && request.target.ends_with("/resume") {
            let mut successor = run("running");
            successor["id"] = json!("successor");
            return Reply::json(201, successor);
        }
        Reply::json(200, run("cancelled"))
    })
    .await;
    let sdk = client(&fixture);
    let run = sdk.run("r");
    let mut request = models::Resume::new(Default::default(), Default::default());
    request.approvals.insert(
        "approval_1".into(),
        models::ApprovalDecision::Approve(Box::new(models::Approve::new(
            models::approve::Action::Approve,
        ))),
    );
    request.calls.insert(
        "call_1".into(),
        models::CallResult::Returned(Box::new(models::Returned::new(
            models::returned::Status::Returned,
            Some(json!({"answers": {"Which?": "A"}})),
        ))),
    );
    let mut payload = a13n::text_payload("Additional evidence");
    payload
        .content
        .push(models::Part::Asset(Box::new(models::AssetPart::new(
            "asset_1".into(),
            models::asset_part::Type::Asset,
        ))));
    request.input = Some(Some(Box::new(payload)));
    let resumed = run.resume(&request, "resume-once").await.unwrap();
    assert_eq!(resumed.run.id, "successor");
    assert_eq!(resumed.receipt.status.as_u16(), 201);
    assert!(matches!(
        resumed.run.interrupt().await,
        Err(Error::Protocol(_))
    ));
    let request = fixture.requests.recv().await.unwrap();
    assert_eq!(request.method, "POST");
    assert!(request.headers.contains("idempotency-key: resume-once"));
    assert_eq!(
        request.json()["approvals"]["approval_1"]["action"],
        "approve"
    );
    assert_eq!(
        request.json()["calls"]["call_1"]["value"]["answers"]["Which?"],
        "A"
    );
    assert_eq!(
        request.json()["input"]["content"][0]["text"],
        "Additional evidence"
    );
    assert_eq!(request.json()["input"]["content"][1]["asset_id"], "asset_1");
    let request = fixture.requests.recv().await.unwrap();
    assert!(request.target.ends_with("/runs/successor/interrupt"));
}

#[tokio::test]
async fn finite_frames_filter_other_runs_and_idle_stream_seals() {
    let mut fixture = server(|request| {
        if request.method == "POST" { return Reply::json(200, receipt()) }
        if request.target.contains("/inbox/e") { return Reply::json(200, entry("consumed", Some("r"))) }
        if request.target.ends_with("/stream") {
            let mut reply = Reply::sse("event: delta\nid: 1-0\ndata: {\"run_id\":\"foreign\",\"attempt\":1,\"sequence\":1,\"event\":{},\"item\":null}\n\nevent: delta\nid: 2-0\ndata: {\"run_id\":\"r\",\"attempt\":1,\"sequence\":1,\"event\":{\"text\":\"hello\"},\"item\":null}\n\n");
            reply.chunks.push((Duration::from_secs(10), vec![b'\n']));
            return reply;
        }
        assert!(request.target.ends_with("/runs/r"));
        Reply::json(200, run("completed"))
    }).await;
    let sdk = client(&fixture);
    let mut interaction = sdk
        .agent("agent_id")
        .send("t", "hello", "once")
        .await
        .unwrap();
    assert!(
        matches!(interaction.next().await.unwrap(), Some(ThreadFrame::Delta { data, .. }) if data.run_id == "r")
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(2), interaction.next())
            .await
            .unwrap()
            .unwrap()
            .is_none()
    );
    assert_eq!(
        *interaction.result().await.unwrap().status(),
        models::RunStatus::Completed
    );
    let mut posts = 0;
    while let Ok(request) = fixture.requests.try_recv() {
        if request.method == "POST" {
            posts += 1
        }
    }
    assert_eq!(posts, 1);
}

#[tokio::test]
async fn early_close_does_not_request_result_or_interrupt_and_failed_entry_is_typed() {
    let fixture = server(|request| {
        if request.method == "POST" {
            return Reply::json(200, receipt());
        }
        if request.target.contains("/inbox/e") {
            return Reply::json(200, entry("failed", None));
        }
        panic!("unexpected request: {}", request.target)
    })
    .await;
    let sdk = client(&fixture);
    let mut interaction = sdk.agent("agent_id").start("hello", "once").await.unwrap();
    assert!(
        matches!(interaction.result().await, Err(Error::Submission(error)) if error.entry.data.status == models::EntryStatus::Failed)
    );
    interaction.close();
    assert!(matches!(interaction.result().await, Err(Error::Closed)));
    assert!(interaction.next().await.unwrap().is_none());
}

#[tokio::test]
async fn completed_before_stream_headers_does_not_block_finite_next() {
    let fixture = server(|request| {
        if request.method == "POST" {
            return Reply::json(200, receipt());
        }
        if request.target.contains("/inbox/e") {
            return Reply::json(200, entry("consumed", Some("r")));
        }
        if request.target.ends_with("/stream") {
            let mut reply = Reply::sse("");
            reply.delay = Duration::from_secs(10);
            return reply;
        }
        Reply::json(200, run("completed"))
    })
    .await;
    let sdk = client(&fixture);
    let mut interaction = sdk.agent("agent_id").start("hello", "once").await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), interaction.next())
            .await
            .unwrap()
            .unwrap()
            .is_none()
    );
    assert_eq!(
        *interaction.result().await.unwrap().status(),
        models::RunStatus::Completed
    );
}

#[tokio::test]
async fn sustained_foreign_frames_cannot_starve_exact_run_seal() {
    let fixture = server(|request| {
        if request.method == "POST" { return Reply::json(200, receipt()) }
        if request.target.contains("/inbox/e") { return Reply::json(200, entry("consumed", Some("r"))) }
        if request.target.ends_with("/stream") {
            let mut reply = Reply::sse("");
            reply.chunks = (0..400).map(|n| (Duration::from_millis(5), format!("event: delta\nid: {n}-0\ndata: {{\"run_id\":\"foreign\",\"attempt\":1,\"sequence\":1,\"event\":{{}},\"item\":null}}\n\n").into_bytes())).collect();
            return reply;
        }
        Reply::json(200, run("completed"))
    }).await;
    let sdk = client(&fixture);
    let mut interaction = sdk.agent("agent_id").start("hello", "once").await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), interaction.next())
            .await
            .unwrap()
            .unwrap()
            .is_none()
    );
    assert_eq!(
        *interaction.result().await.unwrap().status(),
        models::RunStatus::Completed
    );
}

#[tokio::test]
async fn slow_successful_headers_keep_one_attach_while_run_polling() {
    let attached = Arc::new(AtomicUsize::new(0));
    let count = attached.clone();
    let fixture = server(move |request| {
        if request.method == "POST" { return Reply::json(200, receipt()) }
        if request.target.contains("/inbox/e") { return Reply::json(200, entry("consumed", Some("r"))) }
        if request.target.ends_with("/stream") {
            count.fetch_add(1, Ordering::SeqCst);
            let mut reply = Reply::sse("event: delta\nid: 1-0\ndata: {\"run_id\":\"r\",\"attempt\":1,\"sequence\":1,\"event\":{},\"item\":null}\n\n");
            reply.delay = Duration::from_millis(1200);
            return reply;
        }
        Reply::json(200, run("running"))
    }).await;
    let sdk = client(&fixture);
    let mut interaction = sdk.agent("agent_id").start("hello", "once").await.unwrap();
    assert!(
        matches!(tokio::time::timeout(Duration::from_secs(3), interaction.next()).await.unwrap().unwrap(), Some(ThreadFrame::Delta { data, .. }) if data.run_id == "r")
    );
    assert_eq!(attached.load(Ordering::SeqCst), 1);
    interaction.close();
}

#[tokio::test]
async fn start_send_and_message_preserve_complete_configuration_and_ordered_media_payload() {
    let payload = json!({"content":[{"type":"text","text":""},{"type":"url","url":"https://media.example/image.png"},{"type":"url","url":"https://media.example/video.mp4"},{"type":"asset","asset_id":"ast_media"},{"type":"json","value":{"enabled":false,"empty":[],"nested":{"value":null}}}]});
    for configuration in [
        None,
        Some(Value::Null),
        Some(json!({})),
        Some(json!({"allowed_hosts":null})),
        Some(json!({"allowed_hosts":[]})),
        Some(
            json!({"allowed_hosts":["MEDIA.Example.","regex:.*\\.example"],"extensions":{"org.example":{"enabled":false,"zero":0,"empty":{},"array":[null,true,{"deep":[]}],"text":""}}}),
        ),
    ] {
        let mut fixture = server(|_| Reply::json(201, receipt())).await;
        let sdk = client(&fixture);
        let mut options = json!({"overrides":null});
        if let Some(configuration) = configuration {
            options["configuration"] = configuration;
        }
        let options: models::RunOptionsInput = serde_json::from_value(options.clone()).unwrap();
        let expected = serde_json::to_value(&options).unwrap();
        let message: models::MessagePayload = serde_json::from_value(payload.clone()).unwrap();
        let mut interaction = sdk
            .agent("a")
            .start_with(
                message.clone(),
                "start",
                StartOptions {
                    options: Some(Box::new(options.clone())),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        interaction.close();
        let mut interaction = sdk
            .agent("a")
            .send_with(
                "t",
                message.clone(),
                "send",
                a13n::SendOptions {
                    options: Some(Box::new(options.clone())),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        interaction.close();
        let mut request = models::Message::new("a".into(), message);
        request.options = Some(Box::new(options));
        sdk.resources()
            .threads()
            .at("t")
            .inbox()
            .create(
                &request,
                a13n::resources::InboxCreateOptions {
                    idempotency_key: "message".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        for path in [
            "/prefix/api/v1/threads",
            "/prefix/api/v1/threads/t/inbox",
            "/prefix/api/v1/threads/t/inbox",
        ] {
            let sent = fixture.requests.recv().await.unwrap();
            assert_eq!(sent.target, path);
            assert_eq!(sent.json()["options"], expected);
            assert_eq!(sent.json()["payload"], payload);
        }
        assert!(
            fixture.requests.try_recv().is_err(),
            "Closing observation must not interrupt or resend"
        );
    }
}

#[tokio::test]
async fn configuration_conflict_is_service_owned_and_never_retried_or_redirected_to_next_run() {
    let mut fixture = server(|_| Reply::json(409, json!({"error":{"code":"conflict","message":"frozen","details":{"reason":"run_configuration_immutable"}}}))).await;
    let sdk = client(&fixture);
    let options = serde_json::from_value(json!({"configuration":{"allowed_hosts":[]}})).unwrap();
    let error = sdk
        .agent("a")
        .send_with(
            "t",
            "change",
            "once",
            a13n::SendOptions {
                options: Some(Box::new(options)),
                ..Default::default()
            },
        )
        .await
        .err()
        .unwrap();
    assert!(
        matches!(error, Error::Api(ref error) if error.status == 409 && error.details["reason"] == "run_configuration_immutable")
    );
    fixture.requests.recv().await.unwrap();
    assert!(fixture.requests.try_recv().is_err());
}

#[tokio::test]
async fn inline_child_agui_media_and_terminal_observations_do_not_finish_root_interaction() {
    let events = vec![
        json!({"type":"SUBAGENT_STARTED","subagentRunId":"child","parentToolCallId":"delegate","subagentName":"reviewer"}),
        json!({"type":"TEXT_MESSAGE_CONTENT","messageId":"same","subagentRunId":"child","delta":"child"}),
        json!({"type":"TEXT_MESSAGE_CONTENT","messageId":"same","delta":"root"}),
        json!({"type":"TOOL_CALL_RESULT","toolCallId":"call","messageId":"m","role":"tool","subagentRunId":"child","content":[{"type":"text","text":"before"},{"type":"image","source":{"type":"url","value":"https://media.example/i.png","mimeType":"image/png"}},{"type":"image","source":{"type":"url","value":"https://media.example/i.png"}},{"type":"video","source":{"type":"file","value":"file-id","provider":"provider","mimeType":"video/mp4"}}]}),
        json!({"type":"CUSTOM","name":"a13n.input.media","value":{"thread_id":"ht","run_id":"child","sequence":4,"event":{"source":"context","content":{"kind":"binary","payload_omitted":true,"size_bytes":4,"media_type":"image/png"}}},"metadata":{"display":false},"subagentRunId":"child"}),
        json!({"type":"CUSTOM","name":"org.example.native","value":null,"future":{"flag":false,"empty":[]}}),
        json!({"type":"SUBAGENT_FINISHED","subagentRunId":"child"}),
        json!({"type":"RUN_FINISHED","threadId":"ht","runId":"child","outcome":{"type":"success"},"subagentRunId":"child"}),
        json!({"type":"TEXT_MESSAGE_CONTENT","messageId":"same","delta":"after child"}),
    ];
    let wire = events
        .iter()
        .enumerate()
        .map(|(i, event)| {
            format!(
                "event: delta\nid: 100-{}\ndata: {}\n\n",
                i,
                json!({"run_id":"r","attempt":1,"sequence":i,"event":event,"item":null})
            )
        })
        .collect::<String>();
    let served = wire.clone();
    let mut items = sample("RunItems");
    items["run"] = run("completed");
    let mut item = sample("Item");
    item["content"] = json!({"messageId":"same","subagentRunId":"child","result":events[3]["content"],"metadata":{"display":true}});
    items["items"] = json!([item]);
    let expected_items = items.clone();
    let fixture = server(move |request| {
        if request.method == "POST" {
            return Reply::json(201, receipt());
        }
        if request.target.ends_with("/inbox/e") {
            return Reply::json(200, entry("consumed", Some("r")));
        }
        if request.target.ends_with("/stream") {
            return Reply::sse(&served);
        }
        if request.target.ends_with("/items") {
            return Reply::json(200, items.clone());
        }
        Reply::json(200, run("running"))
    })
    .await;
    let sdk = client(&fixture);
    let mut interaction = sdk.agent("a").start("hello", "once").await.unwrap();
    for expected in events {
        let Some(ThreadFrame::Delta { data, .. }) = interaction.next().await.unwrap() else {
            panic!("Child observation terminated root");
        };
        assert_eq!(Value::Object(data.event), expected);
    }
    let items = sdk.run("r").items().get(Default::default()).await.unwrap();
    assert_eq!(serde_json::to_value(items.data).unwrap(), expected_items);
    interaction.close();
}
