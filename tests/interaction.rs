mod common;
use a13n::{Error, generated::models, streaming::ThreadFrame};
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
    let resumed = run
        .resume(&models::ResumeRequest::new(), "resume-once")
        .await
        .unwrap();
    assert_eq!(resumed.run.id, "successor");
    assert_eq!(resumed.receipt.status.as_u16(), 201);
    assert!(matches!(
        resumed.run.interrupt().await,
        Err(Error::Protocol(_))
    ));
    let request = fixture.requests.recv().await.unwrap();
    assert_eq!(request.method, "POST");
    assert!(request.headers.contains("idempotency-key: resume-once"));
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
