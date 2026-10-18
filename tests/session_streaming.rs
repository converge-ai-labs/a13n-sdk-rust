mod common;
use a13n::{
    Error, ProtocolKind,
    streaming::{SessionFrame, SessionStream, SessionStreamOptions},
};
use common::*;
use serde_json::{Value, json};
use std::time::Duration;

fn thread() -> Value {
    json!({"thread":sample("ThreadView"),"current_run":null,"last_run":null,"entries":[]})
}
fn items() -> Value {
    json!({"thread_id":"t","run_id":"r","first":6,"items":[],"continuation":null,"complete":false})
}
fn delta() -> Value {
    json!({"thread_id":"t","run_id":"r","attempt":1,"sequence":2,
        "event":{"type":"CUSTOM","name":"future","value":[false,0,null,[],{"text":"中文"}]},"item":null})
}
fn frame(name: &str, value: Value) -> String {
    format!("event: {name}\ndata: {value}\n\n")
}

#[tokio::test]
async fn session_frames_are_typed_snapshot_based_and_distinct_from_thread_cursors() {
    let body = format!(
        "\u{feff}: heartbeat\r\n\r\nevent: future\ndata: not JSON\n\n{}{}{}{}",
        frame("thread", thread()),
        frame("items", items()),
        frame("delta", delta()),
        frame("gap", json!({"thread_id":"t","run_id":"r"}))
    );
    let expected = delta();
    let mut fixture = server(move |_| Reply::sse(&body)).await;
    let sdk = client(&fixture);
    for _ in 0..2 {
        let mut reader = SessionStream::open(sdk.api().sessions().at("s /"), Default::default())
            .await
            .unwrap();
        let (status, headers) = reader.response().unwrap();
        assert_eq!(status, 200);
        assert_eq!(headers["x-request-id"], "req_test");
        assert!(
            matches!(reader.next().await.unwrap(),Some(SessionFrame::Thread(state)) if state.current_run.is_none() && state.entries.is_empty())
        );
        assert!(
            matches!(reader.next().await.unwrap(),Some(SessionFrame::Items(state)) if state.first==6 && state.continuation.is_none() && !state.complete)
        );
        let Some(SessionFrame::Delta(event)) = reader.next().await.unwrap() else {
            panic!("delta")
        };
        assert_eq!(serde_json::to_value(event).unwrap(), expected);
        assert!(
            matches!(reader.next().await.unwrap(),Some(SessionFrame::Gap(signal)) if signal.thread_id=="t")
        );
        assert!(reader.next().await.unwrap().is_none());
        assert!(reader.next().await.unwrap().is_none());
        let request = fixture.requests.recv().await.unwrap();
        assert_eq!(request.target, "/prefix/api/v1/sessions/s%20%2F/stream");
        assert!(!request.headers.contains("last-event-id"));
        assert!(request.headers.contains("authorization:"));
    }
    assert!(fixture.requests.try_recv().is_err()); // Explicit opens only; no automatic reconnect.
}

#[tokio::test]
async fn session_cancelled_read_retains_partial_utf8_and_close_is_local() {
    let body = frame("delta", delta());
    let split = body.find('中').unwrap() + 1;
    let mut fixture = server(move |_| {
        let mut reply = Reply::sse("");
        reply.chunks = vec![
            (Duration::ZERO, body.as_bytes()[..split].to_vec()),
            (
                Duration::from_millis(150),
                body.as_bytes()[split..].to_vec(),
            ),
        ];
        reply
    })
    .await;
    let sdk = client(&fixture);
    let mut reader = SessionStream::open(sdk.api().sessions().at("s"), Default::default())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), reader.next())
            .await
            .is_err()
    );
    assert!(matches!(
        reader.next().await.unwrap(),
        Some(SessionFrame::Delta(_))
    ));
    reader.close();
    assert!(reader.next().await.unwrap().is_none());
    assert_eq!(fixture.requests.recv().await.unwrap().method, "GET");
    assert!(fixture.requests.try_recv().is_err());
}

#[tokio::test]
async fn known_session_frames_require_nullable_fields_and_reject_ids_wrong_shapes_and_truncation() {
    let mut no_item = delta();
    no_item.as_object_mut().unwrap().remove("item");
    let mut no_continuation = items();
    no_continuation
        .as_object_mut()
        .unwrap()
        .remove("continuation");
    let mut no_current = thread();
    no_current.as_object_mut().unwrap().remove("current_run");
    let mut wrong = items();
    wrong["first"] = json!(false);
    for (body, kind) in [
        (frame("delta", no_item), ProtocolKind::InvalidFrame),
        (frame("items", no_continuation), ProtocolKind::InvalidFrame),
        (frame("thread", no_current), ProtocolKind::InvalidFrame),
        (frame("items", wrong), ProtocolKind::InvalidFrame),
        (
            format!("id: 1-0\n{}", frame("delta", delta())),
            ProtocolKind::InvalidFrame,
        ),
        (
            "event: items\ndata: {}".into(),
            ProtocolKind::IncompleteFrame,
        ),
    ] {
        let fixture = server(move |_| Reply::sse(&body)).await;
        let sdk = client(&fixture);
        let mut reader = SessionStream::open(sdk.api().sessions().at("s"), Default::default())
            .await
            .unwrap();
        assert!(matches!(reader.next().await,Err(Error::Protocol(ref e)) if e.kind==kind));
        assert!(reader.next().await.unwrap().is_none());
    }
    let fixture = server(|_| Reply::sse("event: future\ndata: oversized\n\n")).await;
    let sdk = client(&fixture);
    let mut reader = SessionStream::open(
        sdk.api().sessions().at("s"),
        SessionStreamOptions {
            max_frame_bytes: Some(8),
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(reader.next().await,Err(Error::Protocol(ref e)) if e.kind==ProtocolKind::FrameTooLarge)
    );
}

#[tokio::test]
async fn session_open_preserves_http_errors_content_type_and_parent_cancellation() {
    let fixture = server(|request| match request.target.as_str() {
        "/prefix/api/v1/sessions/denied/stream" => Reply::json(
            403,
            json!({"error":{"code":"forbidden","message":"denied","details":{}}}),
        ),
        "/prefix/api/v1/sessions/wrong/stream" => Reply::json(200, json!({})),
        _ => Reply::sse(&frame("gap", json!({"thread_id":"t","run_id":"r"}))),
    })
    .await;
    let sdk = client(&fixture);
    assert!(
        matches!(SessionStream::open(sdk.api().sessions().at("denied"),Default::default()).await,Err(Error::Api(e)) if e.status==403)
    );
    assert!(
        matches!(SessionStream::open(sdk.api().sessions().at("wrong"),Default::default()).await,Err(Error::Protocol(e)) if e.kind==ProtocolKind::UnexpectedContentType)
    );
    assert!(matches!(
        SessionStream::open(
            sdk.api().sessions().at("s"),
            SessionStreamOptions {
                max_frame_bytes: Some(0)
            }
        )
        .await,
        Err(Error::InvalidInput)
    ));
    let mut reader = SessionStream::open(sdk.api().sessions().at("s"), Default::default())
        .await
        .unwrap();
    sdk.close();
    assert!(matches!(reader.next().await, Err(Error::Closed)));
}

#[tokio::test]
async fn session_nested_required_fields_and_display_bounds_stop_invalid_frames() {
    let mut missing_thread_field = thread();
    missing_thread_field["thread"]
        .as_object_mut()
        .unwrap()
        .remove("current_run_id");
    let mut missing_run_field = thread();
    missing_run_field["current_run"] = sample("RunView");
    missing_run_field["current_run"]
        .as_object_mut()
        .unwrap()
        .remove("output");
    let continuation = json!({"run_id":"r","next_ordinal":1,"position":{"attempt":0,"sequence":0}});
    let mut invalid = vec![
        ("thread", missing_thread_field),
        ("thread", missing_run_field),
    ];
    let mut empty_thread = thread();
    empty_thread["thread"] = json!({});
    invalid.push(("thread", empty_thread));
    let mut empty_item = items();
    empty_item["items"] = json!([{}]);
    invalid.push(("items", empty_item));
    for (field, value) in [
        ("position", json!({})),
        ("position", json!({"attempt":-1,"sequence":0})),
        ("position", json!({"attempt":0,"sequence":-1})),
        ("next_ordinal", json!(0)),
    ] {
        let mut snapshot = items();
        snapshot["continuation"] = continuation.clone();
        snapshot["continuation"][field] = value;
        invalid.push(("items", snapshot));
    }
    for field in [
        "full_content",
        "fragments",
        "memory_sources",
        "observer",
        "response_groups",
    ] {
        let mut snapshot = items();
        snapshot["continuation"] = continuation.clone();
        snapshot["continuation"][field] = Value::Null;
        invalid.push(("items", snapshot));
    }
    for (kind, value) in invalid {
        let body = frame(kind, value);
        let fixture = server(move |_| Reply::sse(&body)).await;
        let sdk = client(&fixture);
        let mut reader = SessionStream::open(sdk.api().sessions().at("s"), Default::default())
            .await
            .unwrap();
        assert!(
            matches!(reader.next().await,Err(Error::Protocol(ref error)) if error.kind==ProtocolKind::InvalidFrame),
            "invalid {kind} accepted"
        );
        assert!(reader.next().await.unwrap().is_none());
    }
}

#[tokio::test]
async fn session_full_snapshots_reuse_canonical_models_and_preserve_skill_policy_unions() {
    for policy in [
        json!({"mode":"shared","overwrite":false,"cleanup":false}),
        json!({"mode":"isolated","cache_count":0}),
    ] {
        let mut state = thread();
        let mut run = sample("RunView");
        run["options"] = json!({"overrides":{"skill_sync":policy.clone()}});
        state["current_run"] = run.clone();
        state["last_run"] = run;
        let mut snapshot = items();
        snapshot["items"] = json!([sample("Item")]);
        snapshot["continuation"] =
            json!({"run_id":"r","next_ordinal":1,"position":{"attempt":0,"sequence":0}});
        let body = format!("{}{}", frame("thread", state), frame("items", snapshot));
        let fixture = server(move |_| Reply::sse(&body)).await;
        let sdk = client(&fixture);
        let mut reader = SessionStream::open(sdk.api().sessions().at("s"), Default::default())
            .await
            .unwrap();
        let Some(SessionFrame::Thread(value)) = reader.next().await.unwrap() else {
            panic!("thread")
        };
        let value = serde_json::to_value(value).unwrap();
        assert_eq!(
            value["current_run"]["options"]["overrides"]["skill_sync"],
            policy
        );
        assert_eq!(
            value["last_run"]["options"]["overrides"]["skill_sync"],
            policy
        );
        assert!(
            matches!(reader.next().await.unwrap(),Some(SessionFrame::Items(value)) if value.continuation.as_ref().unwrap().position.attempt==0 && value.items.len()==1)
        );
        reader.close();
    }
}

#[tokio::test]
async fn session_default_accepts_large_native_snapshots_and_callers_can_bound_frames() {
    let mut item = sample("Item");
    item["content"] = json!({"text":"x".repeat(32_768)});
    let mut snapshot = items();
    snapshot["items"] = json!(vec![item; 40]);
    let body = frame("items", snapshot);
    assert!(body.len() > 1 << 20);
    let fixture = server(move |_| Reply::sse(&body)).await;
    let sdk = client(&fixture);
    let mut reader = SessionStream::open(sdk.api().sessions().at("s"), Default::default())
        .await
        .unwrap();
    assert!(
        matches!(reader.next().await.unwrap(),Some(SessionFrame::Items(value)) if value.items.len()==40)
    );
    reader.close();
    let mut bounded = SessionStream::open(
        sdk.api().sessions().at("s"),
        SessionStreamOptions {
            max_frame_bytes: Some(1 << 20),
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(bounded.next().await,Err(Error::Protocol(ref e)) if e.kind==ProtocolKind::FrameTooLarge)
    );
    assert!(bounded.next().await.unwrap().is_none());
}

#[tokio::test]
async fn session_desktop_wait_preserves_pending_snapshot_without_control() {
    let mut run = sample("RunView");
    run["status"] = json!("waiting");
    run["wait_reason"] = json!("desktop_control");
    run["pending"] = json!({"approvals":[],"calls":[],"desktop_control_id":"desktop"});
    let mut snapshot = thread();
    snapshot["current_run"] = run;
    let body = frame("thread", snapshot.clone());
    let mut fixture = server(move |_| Reply::sse(&body)).await;
    let sdk = client(&fixture);
    let mut reader = SessionStream::open(sdk.api().sessions().at("s"), Default::default())
        .await
        .unwrap();
    let Some(SessionFrame::Thread(value)) = reader.next().await.unwrap() else {
        panic!("thread")
    };
    assert_eq!(serde_json::to_value(&value).unwrap(), snapshot);
    let run = value.current_run.unwrap();
    assert_eq!(
        run.wait_reason,
        Some(a13n::generated::models::WaitReason::DesktopControl)
    );
    assert_eq!(
        run.pending.unwrap().desktop_control_id,
        Some(Some("desktop".into()))
    );
    reader.close();
    assert_eq!(fixture.requests.recv().await.unwrap().method, "GET");
    assert!(fixture.requests.try_recv().is_err());
}
