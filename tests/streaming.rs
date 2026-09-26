mod common;
use a13n::{
    Error,
    streaming::{StreamOptions, ThreadFrame},
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
    let mut stream = client
        .resources()
        .workspaces()
        .at("ws")
        .threads()
        .at("th")
        .events(options())
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
    let mut stream = client
        .resources()
        .workspaces()
        .at("w")
        .threads()
        .at("t")
        .events(options())
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
    let mut stream = client
        .resources()
        .workspaces()
        .at("w")
        .threads()
        .at("t")
        .events(options())
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
    let mut stream = client
        .resources()
        .workspaces()
        .at("w")
        .threads()
        .at("t")
        .events(StreamOptions {
            max_reconnects: 1,
            ..options()
        })
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
    let mut stream = client
        .resources()
        .workspaces()
        .at("w")
        .threads()
        .at("t")
        .events(StreamOptions {
            max_reconnects: 1,
            ..options()
        })
        .await
        .unwrap();
    assert!(stream.next().await.unwrap().is_some());
    assert!(stream.next().await.unwrap().is_some());
    assert!(matches!(stream.next().await, Err(Error::Transport)));
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
    let mut stream = client
        .resources()
        .workspaces()
        .at("w")
        .threads()
        .at("t")
        .events(StreamOptions {
            max_reconnects: 3,
            ..options()
        })
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
        let mut stream = client
            .resources()
            .workspaces()
            .at("w")
            .threads()
            .at("t")
            .events(StreamOptions {
                max_reconnects: 2,
                ..options()
            })
            .await
            .unwrap();
        assert!(
            matches!(stream.next().await, Err(Error::Protocol)),
            "{wire}"
        );
    }
    for body in [vec![0xff, b'\n'], vec![b'x'; 100]] {
        let server = server(move |_| Reply::bytes(200, "text/event-stream", body.clone())).await;
        let client = client(&server);
        let mut stream = client
            .resources()
            .workspaces()
            .at("w")
            .threads()
            .at("t")
            .events(StreamOptions {
                max_frame_bytes: 50,
                ..options()
            })
            .await
            .unwrap();
        assert!(matches!(stream.next().await, Err(Error::Protocol)));
    }
    for status in [401, 403, 404, 409, 500] {
        let mut server = server(move |_| Reply::json(status, serde_json::json!({}))).await;
        let client = client(&server);
        assert!(matches!(
            client
                .resources()
                .workspaces()
                .at("w")
                .threads()
                .at("t")
                .events(StreamOptions {
                    max_reconnects: 3,
                    ..options()
                })
                .await,
            Err(Error::Api(_))
        ));
        server.requests.recv().await.unwrap();
        assert!(server.requests.try_recv().is_err());
    }
}
