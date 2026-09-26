#![allow(dead_code)]
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    task::{JoinHandle, JoinSet},
};
#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: String,
    pub body: Vec<u8>,
}
impl Request {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}
#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: String,
    pub chunks: Vec<(Duration, Vec<u8>)>,
    pub delay: Duration,
    pub disconnect: bool,
}
impl Reply {
    pub fn json(status: u16, body: Value) -> Self {
        Self::bytes(status, "application/json", body.to_string().into_bytes())
    }
    pub fn bytes(status: u16, kind: &str, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: format!("Content-Type: {kind}\r\nETag: \"v1\"\r\nX-Request-ID: req_test\r\n"),
            chunks: vec![(Duration::ZERO, body)],
            delay: Duration::ZERO,
            disconnect: false,
        }
    }
    pub fn sse(body: &str) -> Self {
        Self::bytes(200, "text/event-stream", body.as_bytes().to_vec())
    }
}
pub struct Server {
    pub url: String,
    pub requests: mpsc::UnboundedReceiver<Request>,
    task: JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn more(socket: &mut TcpStream, raw: &mut Vec<u8>) -> bool {
    let mut bytes = [0; 8192];
    let n = socket.read(&mut bytes).await.unwrap_or(0);
    raw.extend_from_slice(&bytes[..n]);
    n > 0
}
async fn serve(
    mut socket: TcpStream,
    sender: mpsc::UnboundedSender<Request>,
    handler: Arc<impl Fn(&Request) -> Reply + Send + Sync + 'static>,
) {
    let mut raw = Vec::new();
    let end = loop {
        if let Some(i) = raw.windows(4).position(|v| v == b"\r\n\r\n") {
            break i + 4;
        }
        if !more(&mut socket, &mut raw).await {
            return;
        }
    };
    let head = String::from_utf8(raw[..end].to_vec()).unwrap();
    let headers = head.to_ascii_lowercase();
    let mut body = Vec::new();
    if headers.contains("transfer-encoding: chunked") {
        let mut at = end;
        loop {
            let length = loop {
                if let Some(i) = raw[at..].windows(2).position(|v| v == b"\r\n") {
                    let n =
                        usize::from_str_radix(std::str::from_utf8(&raw[at..at + i]).unwrap(), 16)
                            .unwrap();
                    at += i + 2;
                    break n;
                }
                if !more(&mut socket, &mut raw).await {
                    return;
                }
            };
            while raw.len() < at + length + 2 {
                if !more(&mut socket, &mut raw).await {
                    return;
                }
            }
            if length == 0 {
                break;
            }
            body.extend_from_slice(&raw[at..at + length]);
            at += length + 2;
        }
    } else {
        let length = headers
            .lines()
            .find_map(|s| s.strip_prefix("content-length: "))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        while raw.len() < end + length {
            if !more(&mut socket, &mut raw).await {
                return;
            }
        }
        body.extend_from_slice(&raw[end..end + length]);
    }
    let mut parts = head.split_whitespace();
    let request = Request {
        method: parts.next().unwrap().into(),
        target: parts.next().unwrap().into(),
        headers,
        body,
    };
    let reply = handler(&request);
    let _ = sender.send(request);
    if reply.disconnect {
        return;
    }
    tokio::time::sleep(reply.delay).await;
    let length: usize = reply.chunks.iter().map(|(_, b)| b.len()).sum();
    let header = format!(
        "HTTP/1.1 {} Test\r\n{}Content-Length: {length}\r\nConnection: close\r\n\r\n",
        reply.status, reply.headers,
    );
    if socket.write_all(header.as_bytes()).await.is_err() {
        return;
    }
    for (delay, bytes) in reply.chunks {
        tokio::time::sleep(delay).await;
        if socket.write_all(&bytes).await.is_err() {
            return;
        }
    }
}
pub async fn server(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/prefix", listener.local_addr().unwrap());
    let (sender, requests) = mpsc::unbounded_channel();
    let handler = Arc::new(handler);
    let task = tokio::spawn(async move {
        let mut children = JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let Ok((socket, _)) = accepted else { break };
                    children.spawn(serve(socket, sender.clone(), handler.clone()));
                },
                Some(result) = children.join_next(), if !children.is_empty() => {
                    result.unwrap();
                }
            }
        }
    });
    Server {
        url,
        requests,
        task,
    }
}
pub fn client(server: &Server) -> a13n::Client {
    a13n::Client::new(&server.url, a13n::Secret::new("test-token")).unwrap()
}
pub fn sample(name: &str) -> Value {
    fn value(schema: &Value, defs: &Value, depth: usize) -> Value {
        if depth > 20 {
            return Value::Null;
        }
        if let Some(reference) = schema["$ref"].as_str() {
            return value(
                &defs[reference.rsplit('/').next().unwrap()],
                defs,
                depth + 1,
            );
        }
        if let Some(v) = schema.get("const") {
            return v.clone();
        }
        if let Some(v) = schema["enum"].as_array() {
            return v[0].clone();
        }
        for union in ["oneOf", "anyOf"] {
            if let Some(branches) = schema[union].as_array() {
                return value(
                    branches
                        .iter()
                        .find(|s| s["type"] != "null")
                        .unwrap_or(&branches[0]),
                    defs,
                    depth + 1,
                );
            }
        }
        match schema["type"].as_str() {
            Some("object") => {
                let mut object = serde_json::Map::new();
                if let Some(required) = schema["required"].as_array() {
                    for key in required {
                        let key = key.as_str().unwrap();
                        object.insert(
                            key.into(),
                            value(&schema["properties"][key], defs, depth + 1),
                        );
                    }
                }
                Value::Object(object)
            }
            Some("array") => json!([]),
            Some("integer") => json!(1),
            Some("number") => json!(1.0),
            Some("boolean") => json!(true),
            Some("null") => Value::Null,
            Some("string") => json!(match schema["format"].as_str() {
                Some("date-time") => "2026-09-26T00:00:00Z",
                Some("date") => "2026-09-26",
                Some("uuid") => "00000000-0000-4000-8000-000000000001",
                _ => "test",
            }),
            _ => json!({}),
        }
    }
    let document: Value = serde_json::from_str(include_str!("../contract/openapi.json")).unwrap();
    value(
        &document["components"]["schemas"][name],
        &document["components"]["schemas"],
        0,
    )
}
