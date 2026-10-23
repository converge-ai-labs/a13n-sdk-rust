//! Independent native Session snapshots and caller-owned Thread progress.
use a13n::{
    Client, Secret,
    streaming::{SessionFrame, SessionStream, StreamOptions, ThreadFrame, ThreadStream},
};
use reqwest::Certificate;
use std::{error::Error as StdError, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
type Result<T> = std::result::Result<T, Box<dyn StdError>>;

pub async fn verify(
    service: &str,
    ca_pem: &[u8],
    token: &str,
    session: &str,
    thread: &str,
    run: &str,
) -> Result<()> {
    let client = Client::builder(service)
        .bearer(Secret::new(token))
        .http_builder(
            reqwest::Client::builder()
                .no_proxy()
                .add_root_certificate(Certificate::from_pem(ca_pem)?),
        )
        .build()?;
    tokio::time::timeout(Duration::from_secs(30), async {
        for _ in 0..2 {
            let mut reader =
                SessionStream::open(client.api().sessions().at(session), Default::default())
                    .await?;
            let (status, headers) = reader.response().ok_or("Missing Session HTTP metadata")?;
            if status != 200 || !headers.contains_key("content-type") {
                return Err("Session HTTP metadata".into());
            }
            verify_snapshot(&mut reader, &client, thread, run).await?;
            if reader.next().await?.is_some() {
                return Err("Session local close".into());
            }
        }
        Ok::<_, Box<dyn StdError>>(())
    })
    .await??;
    client.close();
    println!(
        "Verified HTTPS: authored Session/Thread/Run native snapshots, metadata, bounded close and explicit fresh reopen"
    );
    Ok(())
}

async fn verify_snapshot(
    reader: &mut SessionStream,
    client: &Client,
    thread: &str,
    run: &str,
) -> Result<()> {
    let Some(SessionFrame::Thread(state)) = reader.next().await? else {
        return Err("Session must start with Thread snapshot".into());
    };
    if state.thread.id != thread {
        return Err("Session Thread identity".into());
    }
    if let Some(current) = state.current_run {
        if current.id != run {
            return Err("Session current Run identity".into());
        }
        let Some(SessionFrame::Items(snapshot)) = reader.next().await? else {
            return Err("Session current Run must start with Items".into());
        };
        if snapshot.thread_id != thread || snapshot.run_id != run || snapshot.first < 1 {
            return Err("Session Items identity".into());
        }
        reader.close();
    } else {
        if state.last_run.as_ref().is_none_or(|last| last.id != run) {
            return Err("Session snapshot lost authored sealed Run".into());
        }
        // A fresh connection does not replay Items of an already sealed Run.
        reader.close();
        let saved =
            crate::detailed_items(client.run(run).items().get(Default::default()).await?.data)?;
        if saved.run.id != run || !saved.complete {
            return Err("Sealed Run Items readback".into());
        }
    }
    Ok(())
}

pub async fn offline() -> Result<()> {
    thread_offline().await?;
    fresh_snapshot_offline().await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let client = Client::new(
        &format!("http://{}", listener.local_addr()?),
        Secret::new("offline-token"),
    )?;
    let mut thread = a13n::generated::models::ThreadState::default();
    thread.thread.id = "t".into();
    let frames = format!(
        "event: future\ndata: not-json\n\nevent: thread\ndata: {}\n\nevent: items\ndata: {{\"thread_id\":\"t\",\"run_id\":\"r\",\"first\":6,\"items\":[],\"continuation\":null,\"complete\":true}}\n\nevent: delta\ndata: {{\"thread_id\":\"t\",\"run_id\":\"r\",\"attempt\":1,\"sequence\":2,\"event\":{{\"future\":[false,0,null]}},\"item\":null}}\n\nevent: gap\ndata: {{\"thread_id\":\"t\",\"run_id\":\"r\"}}\n\n",
        serde_json::to_string(&thread)?
    );
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut bytes = [0; 4096];
                let n = socket.read(&mut bytes).await.unwrap();
                assert!(n > 0);
                request.extend_from_slice(&bytes[..n]);
                if request.windows(4).any(|p| p == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with("GET /api/v1/sessions/s/stream "));
            assert!(!request.to_ascii_lowercase().contains("last-event-id"));
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nX-Request-ID: session-test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",frames.len(),frames).as_bytes()).await.unwrap();
        }
    });
    for _ in 0..2 {
        let mut stream =
            SessionStream::open(client.api().sessions().at("s"), Default::default()).await?;
        assert_eq!(stream.response().unwrap().1["x-request-id"], "session-test");
        assert!(
            matches!(stream.next().await?,Some(SessionFrame::Thread(value)) if value.thread.id=="t")
        );
        assert!(
            matches!(stream.next().await?,Some(SessionFrame::Items(value)) if value.complete && value.first==6)
        );
        assert!(
            matches!(stream.next().await?,Some(SessionFrame::Delta(value)) if value.event["future"]==serde_json::json!([false,0,null]) && value.item.is_none())
        );
        assert!(matches!(stream.next().await?,Some(SessionFrame::Gap(value)) if value.run_id=="r"));
        assert!(stream.next().await?.is_none());
        stream.close();
    }
    client.close();
    server.await?;
    println!(
        "Installed crate local TCP: four typed Session frames, unknown event, native nulls, EOF, close and fresh explicit reopen passed"
    );
    Ok(())
}

/// Reopening after seal yields only Thread state; historical Items need a GET.
async fn fresh_snapshot_offline() -> Result<()> {
    use a13n::generated::models::{RunItems, RunStatus, RunView, ThreadState};
    async fn request(socket: &mut tokio::net::TcpStream) -> String {
        let mut data = Vec::new();
        loop {
            let mut bytes = [0; 4096];
            let count = socket.read(&mut bytes).await.unwrap();
            assert!(count > 0);
            data.extend_from_slice(&bytes[..count]);
            if data.windows(4).any(|part| part == b"\r\n\r\n") {
                return String::from_utf8(data).unwrap();
            }
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let client = Client::new(
        &format!("http://{}", listener.local_addr()?),
        Secret::new("offline-token"),
    )?;
    let server = tokio::spawn(async move {
        for sealed in [false, true] {
            let (mut socket, _) = listener.accept().await.unwrap();
            assert!(
                request(&mut socket)
                    .await
                    .starts_with("GET /api/v1/sessions/s/stream ")
            );
            let mut state = ThreadState::default();
            state.thread.id = "t".into();
            let run = RunView {
                id: "r".into(),
                status: if sealed {
                    RunStatus::Completed
                } else {
                    RunStatus::Running
                },
                ..Default::default()
            };
            if sealed {
                state.last_run = Some(Box::new(run));
            } else {
                state.current_run = Some(Box::new(run));
            }
            let mut body = format!(
                "event: thread\ndata: {}\n\n",
                serde_json::to_string(&state).unwrap()
            );
            if !sealed {
                body.push_str("event: items\ndata: {\"thread_id\":\"t\",\"run_id\":\"r\",\"first\":1,\"items\":[],\"continuation\":null,\"complete\":false}\n\n");
            }
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}").as_bytes()).await.unwrap();
            // Keep the stream open: waiting for another frame would time out.
            assert_eq!(socket.read(&mut [0; 1]).await.unwrap(), 0);
            if sealed {
                let (mut socket, _) = listener.accept().await.unwrap();
                assert!(
                    request(&mut socket)
                        .await
                        .starts_with("GET /api/v1/runs/r/items ")
                );
                let mut saved = RunItems::default();
                saved.run.id = "r".into();
                saved.complete = true;
                let body = serde_json::to_string(&saved).unwrap();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
        }
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        for _ in 0..2 {
            let mut reader =
                SessionStream::open(client.api().sessions().at("s"), Default::default()).await?;
            verify_snapshot(&mut reader, &client, "t", "r").await?;
            assert!(reader.next().await?.is_none());
        }
        server.await?;
        Ok::<_, Box<dyn StdError>>(())
    })
    .await??;
    client.close();
    println!(
        "Installed crate local TCP: active Session Items then fresh sealed Thread-only reopen with explicit Run Items readback passed"
    );
    Ok(())
}

/// Prove caller-owned progress query/header wiring from the independently extracted crate.
async fn thread_offline() -> Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let serving = tokio::spawn(async move {
        for index in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut bytes = [0; 4096];
                let count = socket.read(&mut bytes).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&bytes[..count]);
                if request.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8(request).unwrap();
            let (content_type, body): (&str, String) = match index {
                0 => {
                    assert!(text.starts_with("GET /api/v1/threads/t/stream?run=r&position=1-2 "));
                    assert!(text.to_ascii_lowercase().contains("last-event-id: 100-2"));
                    ("text/event-stream", "event: delta\nid: 100-3\ndata: {\"run_id\":\"r\",\"attempt\":1,\"sequence\":3,\"event\":{\"type\":\"TOOL_CALL_RESULT\",\"subagentRunId\":\"child\",\"content\":[{\"type\":\"text\",\"text\":\"child\"},{\"type\":\"binary\",\"mimeType\":\"video/mp4\",\"url\":\"https://media.example/clip.mp4\",\"source\":{\"kind\":\"file\",\"path\":\"clip.mp4\"}},{\"type\":\"binary\",\"mimeType\":\"image/png\",\"metadata\":{\"payload_omitted\":true},\"data\":null}],\"future\":null},\"item\":null}\n\n".into())
                }
                _ => {
                    assert!(text.starts_with("GET /api/v1/threads/t/stream?run=r&position=1-3 "));
                    assert!(text.to_ascii_lowercase().contains("last-event-id: 100-3"));
                    (
                        "text/event-stream",
                        "event: gap\ndata: {\"run_id\":\"r\",\"position\":\"1-5\"}\n\n".into(),
                    )
                }
            };
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
    });
    let client = Client::new(&base, Secret::new("offline-token"))?;
    let mut reader = ThreadStream::open(
        client.api().threads().at("t"),
        StreamOptions {
            run: Some("r".into()),
            position: Some("1-2".into()),
            after: Some("100-2".into()),
            max_reconnects: 1,
            reconnect_delay: Duration::from_millis(1),
            ..Default::default()
        },
    )
    .await?;
    let frame = reader
        .next()
        .await?
        .ok_or("Missing installed native delta")?;
    if !matches!(frame, ThreadFrame::Delta { ref data, .. } if data.event["subagentRunId"] == "child" && data.event["content"][1]["source"]["path"] == "clip.mp4" && data.event["content"][2]["metadata"]["payload_omitted"] == true && data.event.get("future") == Some(&serde_json::Value::Null))
        || reader.applied_position() != Some("1-2")
    {
        return Err("Installed stream acknowledged received rather than applied output".into());
    }
    if !matches!(reader.next().await?, Some(ThreadFrame::Gap(data)) if data.position.as_deref() == Some("1-5"))
        || reader.applied_position() != Some("1-3")
        || reader.applied_cursor() != Some("100-3")
    {
        return Err("Installed stream lost applied coverage or gap recovery target".into());
    }
    reader.close();
    client.close();
    serving.await?;
    println!(
        "Installed crate local TCP: native child/media/null event map, caller-supplied progress/hint, paired applied reconnect and optional gap target passed"
    );
    Ok(())
}
