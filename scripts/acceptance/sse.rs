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
) -> Result<Vec<Option<String>>> {
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
        if !target.ends_with("/stream") {
            return Err("SSE proxy received non-stream route".into());
        }
        let cursor = text
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("last-event-id"))
            .map(|(_, value)| value.trim().to_owned());
        received.push(cursor.clone());
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

pub async fn verify(service: &str, ca_pem: &[u8], token: &str, thread: &str) -> Result<()> {
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
        stream.close();
        client.close();
        Ok::<_, Box<dyn StdError>>(cursors)
    };
    let (received, cursors) = tokio::time::timeout(Duration::from_secs(90), async {
        tokio::try_join!(forwarding, observing)
    })
    .await??;
    if received.len() != 2
        || received[0].is_some()
        || received[1].as_deref() != Some(&cursors[0])
        || cursors[0] == cursors[1]
    {
        return Err("SSE reconnect did not use the first applied cursor".into());
    }
    println!(
        "Verified HTTPS upstream: injected SSE disconnect and applied Last-Event-ID reconnect"
    );
    Ok(())
}
