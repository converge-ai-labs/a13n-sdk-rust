//! Deterministic disconnect over a verified HTTPS upstream Thread event stream.
//! The isolated SDK client connects to a local one-route TCP bridge, which in
//! turn verifies the disposable Service's CA. Other acceptance calls use HTTPS
//! directly. The first upstream stream is truncated after one cursor frame;
//! the second must replay with precisely that applied Last-Event-ID.
use a13n::{
    Client, Secret,
    streaming::{StreamOptions, ThreadFrame, ThreadStream},
};
use reqwest::Certificate;
use std::{error::Error as StdError, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

type Result<T> = std::result::Result<T, Box<dyn StdError>>;
fn frame_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(2)
        .position(|x| x == b"\n\n")
        .map(|i| i + 2)
        .or_else(|| {
            bytes
                .windows(4)
                .position(|x| x == b"\r\n\r\n")
                .map(|i| i + 4)
        })
}
async fn bridge(
    listener: tokio::net::TcpListener,
    http: reqwest::Client,
    upstream: String,
    token: String,
) -> Result<Vec<(String, Option<String>)>> {
    let mut received = Vec::new();
    for attempt in 0..2 {
        let (mut socket, _) = listener.accept().await?;
        let mut request = Vec::new();
        loop {
            let mut chunk = [0; 2048];
            let n = socket.read(&mut chunk).await?;
            if n == 0 || request.len() > 16_384 {
                return Err("Incomplete SSE request".into());
            }
            request.extend_from_slice(&chunk[..n]);
            if request.windows(4).any(|item| item == b"\r\n\r\n") {
                break;
            }
        }
        let text = std::str::from_utf8(&request)?;
        let target = text
            .split_whitespace()
            .nth(1)
            .ok_or("Missing request target")?;
        if !target
            .split('?')
            .next()
            .is_some_and(|path| path.ends_with("/stream"))
        {
            return Err("SSE proxy received non-stream route".into());
        }
        let cursor = text
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("last-event-id"))
            .map(|(_, value)| value.trim().to_owned());
        received.push((target.to_owned(), cursor.clone()));
        let mut outgoing = http
            .get(format!("{}{target}", upstream.trim_end_matches('/')))
            .bearer_auth(&token);
        if let Some(value) = cursor {
            outgoing = outgoing.header("Last-Event-ID", value);
        }
        let mut response = outgoing.send().await?;
        if response.status().as_u16() != 200 {
            return Err(format!("Upstream SSE status {}", response.status()).into());
        }
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await?;
        let mut buffered = Vec::new();
        'stream: loop {
            let chunk = response
                .chunk()
                .await?
                .ok_or("Upstream SSE ended before cursor frame")?;
            buffered.extend_from_slice(&chunk);
            if buffered.len() > 2_000_000 {
                return Err("SSE fixture produced excessive data".into());
            }
            while let Some(end) = frame_end(&buffered) {
                let frame: Vec<u8> = buffered.drain(..end).collect();
                socket
                    .write_all(format!("{:X}\r\n", frame.len()).as_bytes())
                    .await?;
                socket.write_all(&frame).await?;
                socket.write_all(b"\r\n").await?;
                if std::str::from_utf8(&frame)?
                    .lines()
                    .any(|line| line.starts_with("id:"))
                {
                    if attempt == 1 {
                        socket.write_all(b"0\r\n\r\n").await?;
                    }
                    // First close omits the chunk terminator to inject a transport failure.
                    break 'stream;
                }
            }
        }
    }
    Ok(received)
}

pub async fn verify(
    service: &str,
    ca_pem: &[u8],
    token: &str,
    thread: &str,
    run: &str,
) -> Result<()> {
    let direct = Client::builder(service)
        .bearer(Secret::new(token))
        .http_builder(
            reqwest::Client::builder()
                .no_proxy()
                .add_root_certificate(Certificate::from_pem(ca_pem)?),
        )
        .build()?;
    let snapshot = tokio::time::timeout(Duration::from_secs(80), async {
        loop {
            let items = direct.run(run).items().get(Default::default()).await?.data;
            if items.position.is_some() {
                return Ok::<_, a13n::Error>(items);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await??;
    let position = snapshot.position.ok_or("Missing snapshot position")?;
    let hint = snapshot.resume_after.flatten();
    direct.close();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let http = reqwest::Client::builder()
        .no_proxy()
        .add_root_certificate(Certificate::from_pem(ca_pem)?)
        .build()?;
    let forwarding = bridge(listener, http, service.to_owned(), token.to_owned());
    let observing = async {
        let client = Client::new(&url, Secret::new(token))?;
        let handle = client.resources().threads().at(thread);
        let mut stream = ThreadStream::open(
            handle,
            StreamOptions {
                run: Some(snapshot.run.id),
                position: Some(position.clone()),
                after: hint.clone(),
                max_reconnects: 3,
                reconnect_delay: Duration::from_millis(10),
                ..Default::default()
            },
        )
        .await?;
        let mut cursors = Vec::new();
        while cursors.len() < 2 {
            let frame = stream
                .next()
                .await?
                .ok_or("Thread SSE ended before recovery")?;
            if let ThreadFrame::Delta { cursor, .. } | ThreadFrame::Boundary { cursor, .. } = frame
            {
                cursors.push(cursor);
            }
        }
        let applied_position = stream
            .applied_position()
            .map(str::to_owned)
            .ok_or("Lost applied position")?;
        stream.close();
        client.close();
        Ok::<_, Box<dyn StdError>>((cursors, applied_position))
    };
    let (received, (cursors, applied_position)) =
        tokio::time::timeout(Duration::from_secs(90), async {
            tokio::try_join!(forwarding, observing)
        })
        .await??;
    if received.len() != 2
        || received[0].1 != hint
        || !received[0]
            .0
            .contains(&format!("run={run}&position={position}"))
        || !received[1]
            .0
            .contains(&format!("run={run}&position={applied_position}"))
        || received[1].1.as_deref() != Some(&cursors[0])
        || cursors[0] == cursors[1]
    {
        return Err("SSE reconnect did not use the first applied cursor".into());
    }
    println!(
        "Verified HTTPS upstream: Items baseline, injected SSE disconnect and paired applied coverage/cursor reconnect"
    );
    Ok(())
}

/// Prove snapshot query/header wiring from the independently extracted crate.
pub async fn offline() -> Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let serving = tokio::spawn(async move {
        for index in 0..3 {
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
            let (content_type, body) = match index {
                0 => {
                    assert!(text.starts_with("GET /api/v1/runs/r/items "));
                    let mut snapshot = a13n::generated::models::RunItems::default();
                    snapshot.run.id = "r".into();
                    snapshot.position = Some("1-2".into());
                    snapshot.resume_after = Some(Some("100-2".into()));
                    (
                        "application/json",
                        serde_json::to_string(&snapshot).unwrap(),
                    )
                }
                1 => {
                    assert!(text.starts_with("GET /api/v1/threads/t/stream?run=r&position=1-2 "));
                    assert!(text.to_ascii_lowercase().contains("last-event-id: 100-2"));
                    ("text/event-stream", "event: delta\nid: 100-3\ndata: {\"run_id\":\"r\",\"attempt\":1,\"sequence\":3,\"event\":{},\"item\":null}\n\n".into())
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
    let items = client.run("r").items().get(Default::default()).await?.data;
    let mut reader = ThreadStream::open(
        client.resources().threads().at("t"),
        StreamOptions {
            run: Some(items.run.id),
            position: items.position,
            after: items.resume_after.flatten(),
            max_reconnects: 1,
            reconnect_delay: Duration::from_millis(1),
            ..Default::default()
        },
    )
    .await?;
    if !matches!(reader.next().await?, Some(ThreadFrame::Delta { .. }))
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
        "Installed crate local TCP: Items baseline/hint, paired applied reconnect and optional gap target passed"
    );
    Ok(())
}
