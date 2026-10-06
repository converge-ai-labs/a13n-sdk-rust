//! Native ordinal display and sealed-history checks from the extracted crate.
use super::{Result, ensure, key};
use a13n::{
    Client, Error,
    generated::models as m,
    resources::RunItemsGetOptions,
    streaming::{StreamOptions, ThreadFrame, ThreadStream},
};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn snapshot(baseline: bool) -> m::RunItems {
    let mut value = m::RunItems::default();
    value.run.id = "r".into();
    value.run.status = m::RunStatus::Completed;
    value.run.display_position = Some(Some("1-9".into()));
    value.baseline = baseline;
    value.complete = true;
    value.position = baseline.then(|| "1-9".into());
    value.resume_after = Some(baseline.then(|| "100-9".into()));
    value.continuation = Some(if baseline {
        Some(Box::new(serde_json::from_value(json!({"run_id":"r","position":{"attempt":1,"sequence":9},"next_ordinal":10,"full_content":false,"arguments":null,"fragments":{"gap":false,"pending":{"native":{"count":1,"parts":["{}"],"size":2}}},"observer":{"run_id":null,"state":{"children":{"child":{"children":{"nested":{"parts":{},"threads":{},"request_index":0}}}}}}})).unwrap()))
    } else {
        None
    });
    value.items = (6..=if baseline { 9 } else { 7 })
        .map(|ordinal| m::Item {
            ordinal,
            id: format!("i{ordinal}"),
            ..Default::default()
        })
        .collect();
    value
}
pub async fn offline() -> Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let serving = tokio::spawn(async move {
        for index in 0..5 {
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
            let (status, kind, body) = match index {
                0 => {
                    assert!(text.starts_with("GET /api/v1/runs/r/items?limit=2 "));
                    (
                        200,
                        "application/json",
                        serde_json::to_string(&snapshot(true)).unwrap(),
                    )
                }
                1 => {
                    assert!(text.starts_with("GET /api/v1/runs/r/items?before=8&limit=2 "));
                    (
                        200,
                        "application/json",
                        serde_json::to_string(&snapshot(false)).unwrap(),
                    )
                }
                2 => {
                    assert!(text.starts_with("GET /api/v1/runs/r/items?after=0&limit=2 "));
                    (
                        200,
                        "application/json",
                        serde_json::to_string(&snapshot(false)).unwrap(),
                    )
                }
                3 => {
                    assert!(text.starts_with("GET /api/v1/runs/r/items?before=1&after=0 "));
                    (400,"application/json",json!({"error":{"code":"invalid_argument","message":"exclude before/after"}}).to_string())
                }
                _ => {
                    assert!(text.starts_with("GET /api/v1/threads/t/stream "));
                    (200,"text/event-stream","event: delta\nid: 100-1\ndata: {\"run_id\":\"r\",\"attempt\":1,\"sequence\":1,\"event\":{\"type\":\"CUSTOM\",\"subagentRunId\":\"child\",\"value\":null},\"item\":{\"id\":\"i\",\"kind\":\"observation\",\"state\":\"failed\",\"ordinal\":42,\"response_group\":null,\"failure\":{\"future\":[false,null,[]]}}}\n\nevent: delta\nid: 100-2\ndata: {\"run_id\":\"r\",\"attempt\":1,\"sequence\":2,\"event\":{},\"item\":null}\n\nevent: delta\nid: 100-3\ndata: {\"run_id\":\"r\",\"attempt\":1,\"sequence\":3,\"event\":{}}\n\n".into())
                }
            };
            socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
    });
    let client = Client::new(&base, a13n::Secret::new("offline-token"))?;
    let run = client.run("r");
    let tail = run
        .items()
        .get(RunItemsGetOptions {
            limit: Some(2),
            ..Default::default()
        })
        .await?
        .data;
    ensure(
        tail.baseline && tail.complete && tail.items.len() == 4 && tail.items[0].ordinal == 6,
        "Installed whole tail/recent sealed distinction",
    )?;
    ensure(
        serde_json::to_value(&tail)? == serde_json::to_value(snapshot(true))?,
        "Installed recursive continuation roundtrip",
    )?;
    for options in [
        RunItemsGetOptions {
            before: Some(8),
            limit: Some(2),
            ..Default::default()
        },
        RunItemsGetOptions {
            after: Some(0),
            limit: Some(2),
            ..Default::default()
        },
    ] {
        let page = run.items().get(options).await?.data;
        ensure(
            !page.baseline
                && page.complete
                && page.position.is_none()
                && page.continuation == Some(None)
                && page.resume_after == Some(None),
            "Historical window not live coverage",
        )?;
    }
    ensure(
        matches!(run.items().get(RunItemsGetOptions{before:Some(1),after:Some(0),..Default::default()}).await,Err(Error::Api(ref error)) if error.status == 400),
        "Installed Service window error",
    )?;
    let mut reader = ThreadStream::open(
        client.resources().threads().at("t"),
        StreamOptions::default(),
    )
    .await?;
    let Some(ThreadFrame::Delta { data, .. }) = reader.next().await? else {
        return Err("Missing native metadata delta".into());
    };
    ensure(
        data.event["subagentRunId"] == "child"
            && serde_json::to_value(data.item)?
                == json!({"id":"i","kind":"observation","state":"failed","ordinal":42,"response_group":null,"failure":{"future":[false,null,[]]}}),
        "Installed ItemRef metadata",
    )?;
    ensure(
        matches!(reader.next().await?,Some(ThreadFrame::Delta{data,..}) if data.item.is_none()),
        "Installed required null item",
    )?;
    ensure(
        matches!(reader.next().await, Err(Error::Protocol(_))),
        "Installed missing item invalid",
    )?;
    ensure(
        reader.applied_position().is_none(),
        "History must not install coverage",
    )?;
    reader.close();
    serving.await?;
    println!(
        "Installed crate local TCP: ordinal before/after/limit, whole tail > limit, sealed recent/historical distinction, recursive continuation, ItemRef metadata, required nullable item passed"
    );
    Ok(())
}

pub async fn history_followup(
    client: &Client,
    agent: &str,
    thread: &str,
    previous: &str,
) -> Result<()> {
    let read = client
        .resources()
        .threads()
        .at(thread)
        .get(Default::default())
        .await?;
    ensure(
        read.data.current_run_id.is_none() && read.data.last_run_id.as_deref() == Some(previous),
        "Last sealed history pointer",
    )?;
    let mut next = client
        .agent(agent)
        .send(
            thread,
            "Continue after the sealed outcome with new evidence.",
            key(),
        )
        .await?;
    let result = next.result().await?;
    ensure(
        *result.status() == m::RunStatus::Completed
            && result.snapshot.data.parent_run_id.as_deref() == Some(previous),
        "Normal message must continue failed/cancelled history",
    )?;
    Ok(())
}
pub async fn live_window(run: &a13n::Run<'_>) -> Result<()> {
    let recent = run
        .items()
        .get(RunItemsGetOptions {
            limit: Some(1),
            ..Default::default()
        })
        .await?
        .data;
    ensure(
        recent.baseline && recent.complete,
        "Live recent sealed baseline",
    )?;
    for pair in recent.items.windows(2) {
        ensure(
            pair[1].ordinal == pair[0].ordinal + 1,
            "Dense display ordinals",
        )?;
    }
    let historical = run
        .items()
        .get(RunItemsGetOptions {
            after: Some(0),
            limit: Some(1),
            ..Default::default()
        })
        .await?
        .data;
    ensure(
        !historical.baseline
            && historical.complete
            && historical.position.is_none()
            && historical.continuation == Some(None)
            && historical.resume_after == Some(None)
            && historical.items.len() <= 1,
        "Live historical null metadata",
    )?;
    if let Some(first) = historical.items.first() {
        ensure(first.ordinal == 1, "History starts at ordinal one")?;
    }
    let before = run
        .items()
        .get(RunItemsGetOptions {
            before: Some(1),
            limit: Some(1),
            ..Default::default()
        })
        .await?
        .data;
    ensure(
        !before.baseline && before.items.is_empty(),
        "Before first ordinal is empty",
    )?;
    Ok(())
}
