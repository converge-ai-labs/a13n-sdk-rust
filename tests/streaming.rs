mod common;
use a13n::{
    Error,
    streaming::{StreamOptions, ThreadFrame, ThreadStream},
};
use common::*;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
const BOUNDARY: &str =
    "event: boundary\nid: 1-0\ndata: {\"run_id\":\"run_test\",\"attempt\":1,\"sequence\":2}\n\n";
fn options() -> StreamOptions {
    StreamOptions {
        reconnect_delay: Duration::from_millis(1),
        ..Default::default()
    }
}
#[tokio::test]
async fn all_variants_and_cursor_acknowledgement_are_typed() {
    let wire = format!(
        "\u{feff}: heartbeat\r\n\r\nevent: delta\rid: 1-0\rdata: {{\"run_id\":\"run_test\",\"attempt\":1,\"sequence\":1,\"event\":{{\"text\":\"中文\"}},\"item\":{{\"id\":\"i\",\"kind\":\"text_message\",\"state\":\"in_progress\"}}}}\r\r{BOUNDARY}event: changed\ndata: {{\"version\":3}}\n\nevent: gap\ndata: {{\"run_id\":\"r\"}}\n\nevent: reset\ndata: {{\"run_id\":\"r\"}}\n\n"
    );
    let server = server(move |_| Reply::sse(&wire)).await;
    let client = client(&server);
    let mut stream = ThreadStream::open(client.resources().threads().at("th"), options())
        .await
        .unwrap();
    assert_eq!(stream.response().unwrap().0, 200);
    assert!(
        matches!(stream.next().await.unwrap(),Some(ThreadFrame::Delta{data,..}) if data.event["text"]=="中文")
    );
    assert_eq!(stream.last_received_cursor(), Some("1-0"));
    assert_eq!(stream.applied_cursor(), None);
    assert!(matches!(
        stream.next().await.unwrap(),
        Some(ThreadFrame::Boundary { .. })
    ));
    assert_eq!(stream.applied_cursor(), Some("1-0"));
    assert!(matches!(
        stream.next().await.unwrap(),
        Some(ThreadFrame::Changed(_))
    ));
    assert!(matches!(
        stream.next().await.unwrap(),
        Some(ThreadFrame::Gap(_))
    ));
    assert!(matches!(
        stream.next().await.unwrap(),
        Some(ThreadFrame::Reset(_))
    ));
    assert!(stream.next().await.unwrap().is_none());
}
#[tokio::test]
async fn cancellation_keeps_partial_line_utf8_and_frame_state() {
    let body = format!(
        "event: delta\nid: 2-0\ndata: {{\"run_id\":\"r\",\"attempt\":1,\"sequence\":1,\"event\":{{\"text\":\"中文\"}},\"item\":null}}\r\n\r\n{BOUNDARY}"
    );
    let split = body.find('中').unwrap() + 1;
    let server = server(move |_| {
        let mut reply = Reply::sse("");
        reply.chunks = vec![
            (Duration::ZERO, body.as_bytes()[..split].to_vec()),
            (
                Duration::from_millis(100),
                body.as_bytes()[split..].to_vec(),
            ),
        ];
        reply
    })
    .await;
    let client = client(&server);
    let mut stream = ThreadStream::open(client.resources().threads().at("t"), options())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), stream.next())
            .await
            .is_err()
    );
    assert_eq!(stream.applied_cursor(), None);
    assert_eq!(stream.last_received_cursor(), None);
    let frame = stream.next().await.unwrap().unwrap();
    assert_eq!(frame.cursor(), Some("2-0"));
    assert!(matches!(frame,ThreadFrame::Delta{data,..} if data.event["text"]=="中文"));
    stream.close();
    assert_eq!(stream.applied_cursor(), None);
    assert!(stream.next().await.unwrap().is_none());
}
#[tokio::test]
async fn parent_close_rejects_buffered_frames_before_ack() {
    let server = server(|_| Reply::sse(&BOUNDARY.repeat(2))).await;
    let client = client(&server);
    let mut stream = ThreadStream::open(client.resources().threads().at("t"), options())
        .await
        .unwrap();
    assert!(stream.next().await.unwrap().is_some());
    client.close();
    assert!(matches!(stream.next().await, Err(Error::Closed)));
    assert_eq!(stream.applied_cursor(), None);
}
#[tokio::test]
async fn reconnect_sends_only_applied_cursor_and_progress_resets_budget() {
    let count = Arc::new(AtomicUsize::new(0));
    let called = count.clone();
    let mut server = server(move |_| {
        let n = called.fetch_add(1, Ordering::SeqCst);
        Reply::sse(&BOUNDARY.replace("1-0", &format!("{}-0", n + 1)))
    })
    .await;
    let client = client(&server);
    let mut stream = ThreadStream::open(
        client.resources().threads().at("t"),
        StreamOptions {
            max_reconnects: 1,
            ..options()
        },
    )
    .await
    .unwrap();
    for n in 1..=3 {
        assert_eq!(
            stream.next().await.unwrap().unwrap().cursor(),
            Some(format!("{n}-0").as_str())
        );
        let sent = server.requests.recv().await.unwrap();
        if n == 1 {
            assert!(!sent.headers.contains("last-event-id"))
        } else {
            assert!(
                sent.headers
                    .contains(&format!("last-event-id: {}-0", n - 1))
            );
        }
    }
    assert_eq!(stream.applied_cursor(), Some("2-0"));
    stream.close();
    assert_eq!(count.load(Ordering::SeqCst), 3);
}
#[tokio::test]
async fn hints_do_not_reset_retry_budget() {
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let server = server(move |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        Reply::sse("event: gap\ndata: {\"run_id\":\"r\"}\n\n")
    })
    .await;
    let client = client(&server);
    let mut stream = ThreadStream::open(
        client.resources().threads().at("t"),
        StreamOptions {
            max_reconnects: 1,
            ..options()
        },
    )
    .await
    .unwrap();
    assert!(stream.next().await.unwrap().is_some());
    assert!(stream.next().await.unwrap().is_some());
    assert!(matches!(stream.next().await, Err(Error::Transport(_))));
    assert_eq!(count.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn recovery_delay_survives_cancelled_read_and_parent_close() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let mut server = server(move |_| {
        if count.fetch_add(1, Ordering::SeqCst) == 0 {
            Reply::sse(BOUNDARY)
        } else {
            let mut reply = Reply::json(503, serde_json::json!({}));
            reply.headers.push_str("Retry-After: 1\r\n");
            reply
        }
    })
    .await;
    let client = client(&server);
    let mut stream = ThreadStream::open(
        client.resources().threads().at("t"),
        StreamOptions {
            max_reconnects: 3,
            ..options()
        },
    )
    .await
    .unwrap();
    stream.next().await.unwrap();
    server.requests.recv().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), stream.next())
            .await
            .is_err()
    );
    server.requests.recv().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), stream.next())
            .await
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    client.close();
    assert!(matches!(stream.next().await, Err(Error::Closed)));
}
#[tokio::test]
async fn malformed_frames_utf8_limits_and_auth_are_terminal() {
    for wire in [
        "event: boundary\ndata: {\"run_id\":\"r\",\"attempt\":1,\"sequence\":1}\n\n",
        "event: boundary\nid: bad\ndata: {}\n\n",
        "event: changed\nid: 1-0\ndata: {\"version\":1}\n\n",
        "event: delta\nid: 1-0\ndata: {\"run_id\":\"r\",\"attempt\":1,\"sequence\":1,\"event\":{}}\n\n",
        "event: delta\nid: 1-0\ndata: {\"run_id\":\"r\",\"attempt\":1,\"sequence\":1,\"event\":{},\"item\":{\"id\":\"i\",\"kind\":\"unknown\",\"state\":\"completed\"}}\n\n",
        "event: boundary\nid: 1-0\ndata: {\"run_id\":\"r\",\"attempt\":true,\"sequence\":1}\n\n",
        "event: mystery\ndata: {}\n\n",
        "event: changed\ndata: {}\n\n",
        "event: changed\ndata: {\"version\":1}",
    ] {
        let server = server(move |_| Reply::sse(wire)).await;
        let client = client(&server);
        let mut stream = ThreadStream::open(
            client.resources().threads().at("t"),
            StreamOptions {
                max_reconnects: 2,
                ..options()
            },
        )
        .await
        .unwrap();
        assert!(
            matches!(stream.next().await, Err(Error::Protocol(_))),
            "{wire}"
        );
    }
    for body in [vec![0xff, b'\n'], vec![b'x'; 100]] {
        let server = server(move |_| Reply::bytes(200, "text/event-stream", body.clone())).await;
        let client = client(&server);
        let mut stream = ThreadStream::open(
            client.resources().threads().at("t"),
            StreamOptions {
                max_frame_bytes: 50,
                ..options()
            },
        )
        .await
        .unwrap();
        assert!(matches!(stream.next().await, Err(Error::Protocol(_))));
    }
    for status in [401, 403, 404, 409, 500] {
        let mut server = server(move |_| Reply::json(status, serde_json::json!({}))).await;
        let client = client(&server);
        assert!(matches!(
            ThreadStream::open(
                client.resources().threads().at("t"),
                StreamOptions {
                    max_reconnects: 3,
                    ..options()
                }
            )
            .await,
            Err(Error::Api(_))
        ));
        server.requests.recv().await.unwrap();
        assert!(server.requests.try_recv().is_err());
    }
}

fn delta(run: &str, attempt: i64, sequence: i64, cursor: &str) -> String {
    format!(
        "event: delta\nid: {cursor}\ndata: {{\"run_id\":\"{run}\",\"attempt\":{attempt},\"sequence\":{sequence},\"event\":{{\"text\":\"tail\"}},\"item\":null}}\n\n"
    )
}

#[tokio::test]
async fn snapshot_coverage_advances_only_after_apply_and_reconnect_sends_both_positions() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let mut fixture = server(move |_| {
        if count.fetch_add(1, Ordering::SeqCst) == 0 {
            Reply::sse(&delta("r", 1, 3, "100-3"))
        } else {
            Reply::sse("event: gap\ndata: {\"run_id\":\"r\",\"position\":\"1-5\"}\n\n")
        }
    })
    .await;
    let client = common::client(&fixture);
    let mut reader = ThreadStream::open(
        client.resources().threads().at("t"),
        StreamOptions {
            run: Some("r".into()),
            position: Some("1-2".into()),
            after: Some("100-2".into()),
            max_reconnects: 1,
            ..options()
        },
    )
    .await
    .unwrap();
    let initial = fixture.requests.recv().await.unwrap();
    assert!(initial.target.contains("run=r") && initial.target.contains("position=1-2"));
    assert!(initial.headers.contains("last-event-id: 100-2"));
    assert!(matches!(
        reader.next().await.unwrap(),
        Some(ThreadFrame::Delta { .. })
    ));
    assert_eq!(reader.applied_cursor(), Some("100-2"));
    assert_eq!(reader.applied_position(), Some("1-2"));
    let gap = reader.next().await.unwrap().unwrap();
    assert!(matches!(gap, ThreadFrame::Gap(ref data) if data.position.as_deref() == Some("1-5")));
    assert_eq!(reader.applied_cursor(), Some("100-3"));
    assert_eq!(reader.applied_position(), Some("1-3"));
    let reconnect = fixture.requests.recv().await.unwrap();
    assert!(reconnect.target.contains("run=r") && reconnect.target.contains("position=1-3"));
    assert!(reconnect.headers.contains("last-event-id: 100-3"));
    reader.close();
}

#[tokio::test]
async fn gaps_resets_and_foreign_runs_never_advance_claimed_coverage() {
    for signal in [
        "event: gap\ndata: {\"run_id\":\"r\",\"position\":\"1-5\"}\n\n",
        "event: gap\ndata: {\"run_id\":\"r\",\"position\":null}\n\n",
        "event: reset\ndata: {\"run_id\":\"r\"}\n\n",
    ] {
        let wire = format!(
            "{signal}{}{}",
            delta("r", 1, 3, "100-3"),
            delta("foreign", 1, 4, "100-4")
        );
        let fixture = server(move |_| Reply::sse(&wire)).await;
        let client = common::client(&fixture);
        let mut reader = ThreadStream::open(
            client.resources().threads().at("t"),
            StreamOptions {
                run: Some("r".into()),
                position: Some("1-2".into()),
                ..options()
            },
        )
        .await
        .unwrap();
        reader.next().await.unwrap();
        reader.next().await.unwrap();
        reader.next().await.unwrap();
        assert_eq!(reader.applied_position(), Some("1-2"));
        assert_eq!(reader.applied_cursor(), Some("100-3"));
        reader.close();
    }
    let fixture = server(move |_| Reply::sse(&delta("r", 1, 3, "100-3"))).await;
    let client = common::client(&fixture);
    let mut unclaimed = ThreadStream::open(client.resources().threads().at("t"), options())
        .await
        .unwrap();
    unclaimed.next().await.unwrap();
    unclaimed.next().await.unwrap();
    assert_eq!(unclaimed.applied_position(), None);
}

#[tokio::test]
async fn covered_boundary_and_drop_do_not_ack_new_position() {
    let wire = format!(
        "event: boundary\nid: 100-2\ndata: {{\"run_id\":\"r\",\"attempt\":1,\"sequence\":2}}\n\n{}",
        delta("r", 1, 3, "100-3")
    );
    let fixture = server(move |_| Reply::sse(&wire)).await;
    let client = common::client(&fixture);
    let mut reader = ThreadStream::open(
        client.resources().threads().at("t"),
        StreamOptions {
            run: Some("r".into()),
            position: Some("1-2".into()),
            ..options()
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        reader.next().await.unwrap(),
        Some(ThreadFrame::Boundary { .. })
    ));
    reader.next().await.unwrap();
    assert_eq!(reader.applied_cursor(), Some("100-2"));
    assert_eq!(reader.applied_position(), Some("1-2"));
    reader.close();
    assert!(reader.next().await.unwrap().is_none());
    assert_eq!(reader.applied_position(), Some("1-2"));
}

#[tokio::test]
async fn coverage_options_and_gap_position_validate_only_wire_shape() {
    let mut fixture = server(|_| Reply::sse("")).await;
    let client = common::client(&fixture);
    for (run, position) in [
        (Some("r"), None),
        (None, Some("1-0")),
        (Some(""), Some("1-0")),
        (Some("r"), Some("01-0")),
        (Some("r"), Some("1-x")),
        (Some("r"), Some("1-000")),
    ] {
        let result = ThreadStream::open(
            client.resources().threads().at("t"),
            StreamOptions {
                run: run.map(str::to_owned),
                position: position.map(str::to_owned),
                ..options()
            },
        )
        .await;
        assert!(matches!(result, Err(Error::InvalidInput)));
    }
    assert!(fixture.requests.try_recv().is_err());
    for payload in [
        "{\"run_id\":\"r\"}",
        "{\"run_id\":\"r\",\"position\":null}",
        "{\"run_id\":\"r\",\"position\":\"12345678901234567890-0\"}",
    ] {
        let wire = format!("event: gap\ndata: {payload}\n\n");
        let fixture = server(move |_| Reply::sse(&wire)).await;
        let client = common::client(&fixture);
        let mut reader = ThreadStream::open(client.resources().threads().at("t"), options())
            .await
            .unwrap();
        assert!(matches!(
            reader.next().await.unwrap(),
            Some(ThreadFrame::Gap(_))
        ));
    }
}

#[tokio::test]
async fn sequence_holes_boundaries_and_attempt_changes_freeze_coverage_until_reopen() {
    for first in [
        delta("r", 1, 4, "100-4"),
        delta("r", 2, 3, "100-4"),
        "event: boundary\nid: 100-4\ndata: {\"run_id\":\"r\",\"attempt\":1,\"sequence\":4}\n\n"
            .into(),
    ] {
        let wire = format!("{first}{}", delta("r", 1, 3, "100-5"));
        let fixture = server(move |_| Reply::sse(&wire)).await;
        let client = client(&fixture);
        let mut reader = ThreadStream::open(
            client.resources().threads().at("t"),
            StreamOptions {
                run: Some("r".into()),
                position: Some("1-2".into()),
                ..options()
            },
        )
        .await
        .unwrap();
        reader.next().await.unwrap();
        reader.next().await.unwrap();
        reader.next().await.unwrap();
        assert_eq!(reader.applied_position(), Some("1-2"));
        assert_eq!(reader.applied_cursor(), Some("100-5"));
    }
}

#[tokio::test]
async fn coverage_with_missing_or_stale_hint_accepts_filtered_replay_without_local_gap() {
    for hint in [None, Some("1-0")] {
        let mut fixture = server(|_| Reply::sse(&format!("{}event: boundary\nid: 100-3\ndata: {{\"run_id\":\"r\",\"attempt\":1,\"sequence\":3}}\n\n", delta("r", 1, 3, "100-3")))).await;
        let client = client(&fixture);
        let mut reader = ThreadStream::open(
            client.resources().threads().at("t"),
            StreamOptions {
                run: Some("r".into()),
                position: Some("1-2".into()),
                after: hint.map(str::to_owned),
                ..options()
            },
        )
        .await
        .unwrap();
        let request = fixture.requests.recv().await.unwrap();
        assert!(request.target.ends_with("?run=r&position=1-2"));
        assert_eq!(request.headers.contains("last-event-id:"), hint.is_some());
        assert!(matches!(
            reader.next().await.unwrap(),
            Some(ThreadFrame::Delta { .. })
        ));
        assert!(matches!(
            reader.next().await.unwrap(),
            Some(ThreadFrame::Boundary { .. })
        ));
        assert_eq!(reader.applied_position(), Some("1-3"));
    }
}
