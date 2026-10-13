//! Current contract journeys against the installed crate only.
use super::{Result, ensure, key};
use a13n::{Client, Overrides, RunOptions, StartOptions, generated::models as m, resources::*};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const HISTORY: &str = r#"[{"parts":[{"content":"Earlier","part_kind":"user-prompt"}],"kind":"request"},{"parts":[{"args":{"z":0,"a":{"second":2,"first":1}},"tool_call_id":"c","tool_name":"lookup","part_kind":"tool-call"}],"kind":"response"},{"parts":[{"content":{"z":[],"a":false},"tool_call_id":"c","tool_name":"lookup","part_kind":"tool-return"}],"kind":"request"}]"#;

pub async fn offline() -> Result<()> {
    let history: Vec<serde_json::Map<String, Value>> = serde_json::from_str(HISTORY)?;
    let mut body = m::NewThread::new("a".into(), a13n::text_payload("Now"));
    body.message_history = Some(history.clone());
    ensure(
        serde_json::to_string(&body)?.contains(HISTORY),
        "Installed native history reordered",
    )?;
    let mut receipt = m::Submitted::default();
    receipt.thread.id = "t".into();
    receipt.thread.workspace_id = "w".into();
    receipt.thread.message_history = history.clone();
    receipt.entry.thread_id = "t".into();
    receipt.entry.id = "e".into();
    receipt.run = None;
    let thread = serde_json::to_string(&receipt.thread)?;
    let receipt = serde_json::to_string(&receipt)?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let client = Client::new(
        &format!("http://{}", listener.local_addr()?),
        a13n::Secret::new("offline-token"),
    )?;
    let server = tokio::spawn(async move {
        for (expected, status, response) in [
            ("POST /api/v1/threads ", "201 Created", receipt.as_str()),
            ("GET /api/v1/threads/t ", "200 OK", thread.as_str()),
            (
                "GET /api/v1/runs/r/contents/c ",
                "200 OK",
                r#"{"id":"c","media_type":"application/json","value":{"z":false,"a":[null,0]},"truncated":false}"#,
            ),
            (
                "GET /api/v1/sessions/s/message-authors?entry_id=e ",
                "200 OK",
                r#"{"items":[{"entry_id":"e","principal_id":"p","principal":null,"submitted_at":"2026-10-13T00:00:00Z"}]}"#,
            ),
        ] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0; 4096];
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&chunk[..count]);
                let request = String::from_utf8_lossy(&bytes);
                if let Some((headers, body)) = request.split_once("\r\n\r\n") {
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(|v| v.parse().unwrap())
                        })
                        .unwrap_or(0);
                    if body.len() >= length {
                        break;
                    }
                }
            }
            let request = String::from_utf8(bytes).unwrap();
            assert!(request.starts_with(expected));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer offline-token")
            );
            if expected.starts_with("POST") {
                assert!(request.contains(HISTORY));
            }
            socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).as_bytes()).await.unwrap();
        }
    });
    let interaction = client
        .agent("a")
        .start_with(
            "Now",
            StartOptions {
                message_history: Some(history),
                ..Default::default()
            },
        )
        .await?;
    ensure(
        serde_json::to_string(
            &interaction
                .evidence
                .submission()
                .unwrap()
                .data
                .thread
                .message_history,
        )? == HISTORY,
        "Installed receipt history order",
    )?;
    let readback = client
        .api()
        .threads()
        .at("t")
        .get(Default::default())
        .await?;
    ensure(
        serde_json::to_string(&readback.data.message_history)? == HISTORY,
        "Installed readback history order",
    )?;
    let content = client
        .api()
        .runs()
        .at("r")
        .contents()
        .at("c")
        .get(Default::default())
        .await?;
    ensure(
        content.data.value == Some(json!({"z":false,"a":[null,0]})),
        "Installed content native JSON",
    )?;
    let authors = client
        .api()
        .sessions()
        .at("s")
        .message_authors()
        .list(SessionMessageAuthorsListOptions {
            entry_id: vec!["e".into()],
            ..Default::default()
        })
        .await?;
    ensure(
        authors.data.items[0].entry_id == "e",
        "Installed author attribution",
    )?;
    let evidence = json!({"id":"item","kind":"text_message","state":"completed","memory_sources":[{"kind":"file","memory":"notes","status":"full","files":[{"path":"note.md","sha256":"digest","bytes":0,"truncated":false}]}]});
    let typed: a13n::streaming::ItemRef = serde_json::from_value(evidence.clone())?;
    ensure(
        serde_json::to_value(typed)? == evidence,
        "Installed memory evidence",
    )?;
    for policy in [
        json!({"mode":"shared","overwrite":false,"cleanup":false}),
        json!({"mode":"isolated","cache_count":0}),
    ] {
        let typed = serde_json::from_value(policy.clone())?;
        ensure(
            serde_json::to_value(
                Overrides::new()
                    .lazy_environment(Some(false))
                    .skill_sync(Some(typed)),
            )? == json!({"lazy_environment":false,"skill_sync":policy}),
            "Installed overrides presence",
        )?;
    }
    client.close();
    server.await?;
    println!(
        "Installed crate local TCP: ordered authored/native history and readback, explicit content/authors, typed memory evidence and skill policies passed"
    );
    Ok(())
}

pub async fn live(client: &Client, agent: &str) -> Result<()> {
    let text = "Saved display Unicode evidence: ".to_owned() + &"語".repeat(16000);
    let mut interaction = client
        .agent(agent)
        .start_with(
            text.clone(),
            StartOptions {
                idempotency_key: Some(key()),
                run_options: Some(
                    RunOptions::new().overrides(Some(
                        Overrides::new()
                            .lazy_environment(Some(false))
                            .skill_sync(Some(serde_json::from_value(
                                json!({"mode":"isolated","cache_count":0}),
                            )?)),
                    )),
                ),
                ..Default::default()
            },
        )
        .await?;
    let submitted = &interaction.evidence.submission().unwrap().data;
    let session = submitted.thread.session_id.clone();
    let entry = submitted.entry.id.clone();
    let outcome = interaction.result().await?;
    ensure(
        outcome.status == m::RunStatus::Completed,
        "Content journey not completed",
    )?;
    let items = outcome.run.items().get(Default::default()).await?;
    let reference = items
        .data
        .items
        .iter()
        .filter_map(|item| item.content_refs.as_ref())
        .flat_map(|refs| refs.values())
        .find(|reference| reference.size_bytes as usize == text.len())
        .ok_or("Missing committed large Unicode reference")?;
    let content = client
        .api()
        .runs()
        .at(&outcome.run_id)
        .contents()
        .at(&reference.id)
        .get(Default::default())
        .await?;
    ensure(
        content.data.value == Some(json!(text)) && content.data.truncated == Some(false),
        "Full Unicode content/truncation",
    )?;
    ensure(
        content
            .headers
            .get("cache-control")
            .is_some_and(|h| h == "no-store"),
        "Content cache policy",
    )?;
    let authors = client
        .api()
        .sessions()
        .at(&session)
        .message_authors()
        .list(SessionMessageAuthorsListOptions {
            entry_id: vec![entry.clone(), "ent_missing".into()],
            ..Default::default()
        })
        .await?;
    ensure(
        authors.data.items.len() == 1 && authors.data.items[0].entry_id == entry,
        "Exact Entry authors",
    )?;
    let history = client
        .agent(agent)
        .start_with(
            "Continue ordered imported history",
            StartOptions {
                message_history: Some(serde_json::from_str(HISTORY)?),
                ..Default::default()
            },
        )
        .await?;
    let readback = history
        .thread
        .as_ref()
        .ok_or("Missing imported Thread")?
        .resource()
        .get(Default::default())
        .await?;
    ensure(
        serde_json::to_string(&readback.data.message_history)? == HISTORY,
        "Service immutable history order",
    )?;
    println!(
        "Verified HTTPS: 48KB Unicode ContentRef/read/no-store, exact Session authors, lazy/isolated policy and ordered imported history"
    );
    Ok(())
}
