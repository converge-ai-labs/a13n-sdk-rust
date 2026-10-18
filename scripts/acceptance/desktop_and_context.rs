//! Installed-artifact boundaries; HTTPS uses only the caller's disposable fixture.
use super::{Result, ensure, key, required};
use a13n::{Client, Error, generated::models as m, resources::*};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub async fn offline() -> Result<()> {
    for wire in [
        json!({"mode":"native"}),
        json!({"mode":"native","strategy":null}),
        json!({"mode":"native","strategy":"regex"}),
        json!({"mode":"proxy","max_results":7}),
    ] {
        let typed: m::ToolSearchConfig = serde_json::from_value(wire.clone())?;
        ensure(
            serde_json::to_value(typed)? == wire,
            "ToolSearch union roundtrip",
        )?;
    }
    for wire in [
        json!({"mode":"native","max_results":7}),
        json!({"mode":"proxy","strategy":"regex"}),
        json!({}),
    ] {
        ensure(
            serde_json::from_value::<m::ToolSearchConfig>(wire).is_err(),
            "Invalid union accepted",
        )?;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let client = Client::new(
        &format!("http://{}", listener.local_addr()?),
        a13n::Secret::new("offline-token"),
    )?;
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut data = [0; 8192];
            let size = socket.read(&mut data).await.unwrap();
            let input = String::from_utf8_lossy(&data[..size]);
            assert!(
                input.starts_with("GET /api/v1/usage/context?record_id=r ")
                    || input.starts_with("GET /api/v1/runs/r/desktop-control ")
            );
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Request-ID: null-evidence\r\nContent-Length: 4\r\nConnection: close\r\n\r\nnull").await.unwrap();
        }
    });
    let usage = client
        .api()
        .usage()
        .context()
        .get(UsageContextGetOptions {
            record_id: Some("r".into()),
            ..Default::default()
        })
        .await?;
    ensure(
        usage.data.is_none() && usage.status == 200 && usage.request_id() == Some("null-evidence"),
        "Nullable context evidence",
    )?;
    ensure(
        client
            .api()
            .runs()
            .at("r")
            .desktop_control()
            .get(Default::default())
            .await?
            .data
            .is_none(),
        "Nullable desktop metadata",
    )?;
    server.await?;
    println!(
        "Installed crate strict ToolSearch and nullable context/desktop mock boundaries passed"
    );
    Ok(())
}

pub async fn live(client: &Client, agent: &str) -> Result<()> {
    if let Ok(record) = std::env::var("A13N_MODEL_RECORD") {
        let context = client
            .api()
            .usage()
            .context()
            .get(UsageContextGetOptions {
                record_id: Some(record),
                ..Default::default()
            })
            .await?;
        ensure(
            context.status == 200 && context.request_id().is_some(),
            "Context HTTP evidence",
        )?;
        let usage = context.data.ok_or("Model record has no contribution")?;
        ensure(
            usage.input_tokens.is_some()
                && usage.output_tokens.is_some()
                && usage.context_window_tokens.is_none(),
            "Context nullable window",
        )?;
        let run = client
            .api()
            .runs()
            .at(&usage.run_id)
            .get(Default::default())
            .await?;
        for options in [
            UsageContextGetOptions {
                run_id: Some(usage.run_id.clone()),
                ..Default::default()
            },
            UsageContextGetOptions {
                thread_id: Some(run.data.thread_id),
                ..Default::default()
            },
        ] {
            ensure(
                client.api().usage().context().get(options).await?.data == Some(usage.clone()),
                "Context selectors disagree",
            )?;
        }
        for options in [
            UsageContextGetOptions::default(),
            UsageContextGetOptions {
                run_id: Some(usage.run_id.clone()),
                record_id: Some(usage.record_id.clone()),
                ..Default::default()
            },
        ] {
            ensure(
                matches!(client.api().usage().context().get(options).await,Err(Error::Api(e)) if e.status==400),
                "Invalid selectors",
            )?;
        }
        ensure(
            matches!(client.api().usage().context().get(UsageContextGetOptions{run_id:Some(format!("run_{}","0".repeat(24))),..Default::default()}).await,Err(Error::Api(e)) if e.status==404),
            "Missing Run context",
        )?;
        ensure(
            client
                .api()
                .runs()
                .at(&usage.run_id)
                .desktop_control()
                .get(Default::default())
                .await?
                .data
                .is_none(),
            "Absent desktop metadata",
        )?;
    }
    if let Ok(prompt) = std::env::var("A13N_FAILURE_PROMPT") {
        let mut interaction = client.agent(agent).start(prompt).await?;
        let failed = interaction.result().await?;
        ensure(
            failed.status == m::RunStatus::Failed,
            "Failure fixture did not fail",
        )?;
        ensure(
            client
                .api()
                .usage()
                .context()
                .get(UsageContextGetOptions {
                    run_id: Some(failed.run_id),
                    ..Default::default()
                })
                .await?
                .data
                .is_none(),
            "No contribution is JSON null",
        )?;
    }
    let model = required("A13N_MODEL_KEY")?;
    for config in [
        json!({"mode":"proxy","search_name":"find_grouped_tools","call_name":"invoke_grouped_tool","max_results":7}),
        json!({"mode":"native"}),
        json!({"mode":"native","strategy":null}),
        json!({"mode":"native","strategy":"regex"}),
    ] {
        let body: m::AgentCreate = serde_json::from_value(
            json!({"name":format!("ToolSearch {}",key()),"config":{"model":model,"tool_search":{"config":config,"groups":{}}}}),
        )?;
        let created = client
            .api()
            .agents()
            .create(&body, Default::default())
            .await?;
        let revision = client
            .api()
            .agents()
            .at(&created.data.id)
            .revisions()
            .at(created
                .data
                .default_revision_id
                .as_deref()
                .ok_or("Created Agent has no default Revision")?)
            .get(Default::default())
            .await?;
        let saved = serde_json::to_value(revision.data.config)?;
        ensure(
            saved["tool_search"]["groups"] == json!({}),
            "Empty groups lost",
        )?;
        for (name, value) in config.as_object().unwrap() {
            ensure(
                saved["tool_search"]["config"][name] == *value,
                "Stored ToolSearch branch changed",
            )?;
        }
    }
    println!(
        "Installed crate HTTPS ToolSearch revisions, context selectors and null/absence passed"
    );
    Ok(())
}
