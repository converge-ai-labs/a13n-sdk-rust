//! Independent executable consuming only the extracted a13n .crate archive.
mod sse;
use a13n::{
    Client, Error, ProtocolKind, Secret, TransportKind, TransportStage, UploadFile,
    generated::models as m, resources::*, text_payload,
};
use std::{
    env,
    error::Error as StdError,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

type Result<T> = std::result::Result<T, Box<dyn StdError>>;
fn ensure(ok: bool, description: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(description.to_owned().into())
    }
}
fn required(name: &str) -> Result<String> {
    let value = env::var(name)?;
    ensure(!value.is_empty(), &format!("{name} must not be empty"))?;
    Ok(value)
}
fn key() -> String {
    format!(
        "rust-sdk-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}
fn bound<'a>(client: &'a Client, raw: a13n::Response<m::Submitted>) -> Result<a13n::Submitted<'a>> {
    Ok(a13n::Submitted::bind(client, raw)?)
}
fn accepted<'a>(submitted: &'a a13n::Submitted<'_>) -> Result<&'a a13n::Run<'a>> {
    submitted
        .run
        .as_ref()
        .ok_or_else(|| "Accepted submission has no Run".into())
}
async fn wait(run: &a13n::Run<'_>, expected: m::RunStatus) -> Result<()> {
    let result = tokio::time::timeout(
        Duration::from_secs(80),
        run.wait_with(Duration::from_secs(80), Duration::from_millis(100)),
    )
    .await??;
    ensure(
        result.snapshot.data.status == expected,
        &format!("Unexpected Run status: {:?}", result.snapshot.data.status),
    )?;
    let items = run.items().get(Default::default()).await?;
    ensure(
        items.data.run.id == result.snapshot.data.id && items.data.complete,
        "Run Items mismatch",
    )
}

// Minimal offline TCP origin: enough to independently prove route, paging,
// HTTP evidence, structured error and close without a Service or fixture secret.
async fn offline() -> Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base_url = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move {
        for _ in 0..4 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut data = [0; 8192];
            let size = socket.read(&mut data).await.unwrap();
            let input = String::from_utf8_lossy(&data[..size]);
            assert!(
                input.contains("authorization: Bearer offline-token")
                    || input.contains("Authorization: Bearer offline-token")
            );
            let (status, body, extra) = if input.starts_with("GET /api/v1/threads") {
                (
                    "200 OK",
                    r#"{"items":[],"next_cursor":null}"#,
                    "ETag: \"offline\"\r\n",
                )
            } else if input.starts_with("GET /api/v1/organizations/offline/members?") {
                assert!(input.contains("kind=service_account"));
                ("200 OK", r#"{"items":[],"next_cursor":null}"#, "")
            } else if input.starts_with("GET /api/v1/workspaces/invalid") {
                ("200 OK", "private-response-body", "")
            } else {
                assert!(input.starts_with("GET /api/v1/workspaces/fail"));
                (
                    "428 Precondition Required",
                    r#"{"error":{"code":"precondition_required","message":"missing If-Match","details":{"header":"If-Match"}}}"#,
                    "",
                )
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra}X-Request-ID: offline-request\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        }
    });
    let client = Client::new(&base_url, Secret::new("offline-token"))?;
    let resources = client.resources();
    let mut pages = resources.threads().pages(ThreadsListOptions::default());
    let first = pages.next().await?.ok_or("Missing first page")?;
    ensure(
        first.status.as_u16() == 200
            && first.etag() == Some("\"offline\"")
            && first.request_id() == Some("offline-request"),
        "HTTP metadata",
    )?;
    ensure(
        first.data.items.is_empty() && pages.next().await?.is_none(),
        "Page iteration",
    )?;
    let error = resources
        .workspaces()
        .at("fail")
        .get()
        .await
        .expect_err("Expected 428");
    ensure(
        matches!(error, Error::Api(ref value) if value.status == 428 && value.code == "precondition_required"),
        "Structured ApiError",
    )?;
    resources
        .organizations()
        .at("offline")
        .members()
        .list(OrganizationMembersListOptions {
            kind: Some(MemberKind::ServiceAccount),
            ..Default::default()
        })
        .await?;
    ensure(
        ProviderKind::Memory.as_str() == "memory" && SkillsSource::Github.to_string() == "github",
        "Domain enum wire values",
    )?;
    let error = resources
        .workspaces()
        .at("invalid")
        .get()
        .await
        .unwrap_err();
    ensure(
        matches!(&error, Error::Protocol(info)
            if info.kind == ProtocolKind::InvalidJson
            && info.status == Some(200)
            && info.request_id.as_deref() == Some("offline-request")),
        "Typed protocol diagnostics",
    )?;
    for text in [error.to_string(), format!("{error:?}")] {
        ensure(
            !text.contains("private-response-body") && !text.contains("offline-request"),
            "Protocol diagnostic redaction",
        )?;
    }
    let source = error.source().ok_or("Missing safe diagnostic source")?;
    ensure(source.source().is_none(), "Raw diagnostic cause retained")?;
    let error = Client::builder("http://localhost/private")
        .http_builder(reqwest::Client::builder().user_agent("private\nvalue"))
        .build()
        .err()
        .ok_or("Expected builder error")?;
    ensure(
        matches!(&error, Error::Transport(info)
            if info.stage == TransportStage::Build && info.kind == TransportKind::Builder),
        "Typed transport diagnostics",
    )?;
    ensure(
        !format!("{error:?}").contains("private"),
        "Transport redaction",
    )?;
    let null = m::AgentUpdate {
        description: Some(None),
        ..Default::default()
    };
    ensure(
        serde_json::to_string(&null)? == r#"{"description":null}"#,
        "Nullable omission/null distinction",
    )?;
    ensure(
        serde_json::to_string(&m::NewThread::new("agent".into(), text_payload("hello")))?
            .contains("\"payload\""),
        "Typed rich submission",
    )?;
    ensure(
        format!("{:?}", Secret::new("offline-token")).contains("[REDACTED]"),
        "Secret redaction",
    )?;
    client.close();
    ensure(
        matches!(
            resources.workspaces().at("offline").get().await,
            Err(Error::Closed)
        ),
        "Parent shutdown",
    )?;
    server.await?;
    println!(
        "Installed crate local TCP: typed resource/enums/diagnostics, redaction, metadata, pagination, 428, nullable and close passed"
    );
    Ok(())
}

async fn live() -> Result<()> {
    let ca = std::fs::read(required("A13N_CA_BUNDLE")?)?;
    let cert = reqwest::Certificate::from_pem(&ca)?;
    let http = reqwest::Client::builder()
        .no_proxy()
        .add_root_certificate(cert);
    let service = required("A13N_SERVICE_URL")?;
    let token = required("A13N_API_TOKEN")?;
    let client = Client::builder(service.clone())
        .bearer(Secret::new(token.clone()))
        .http_builder(http)
        .build()?;
    let resources = client.resources();
    let workspace_id = required("A13N_WORKSPACE")?;
    let ws = client.resources();
    let agent = required("A13N_AGENT")?;
    ensure(
        resources.healthz().get().await?.status.as_u16() == 200,
        "HTTPS health",
    )?;
    ensure(
        resources.readyz().get().await?.status.as_u16() == 200,
        "HTTPS ready",
    )?;
    ensure(
        resources
            .auth()
            .configuration()
            .get()
            .await?
            .data
            .initialized,
        "HTTPS auth configuration",
    )?;
    let canonical = resources.workspaces().at(&workspace_id).get().await?.data;
    let key_ws = client.resources();
    let request = m::NewThread::new(
        agent.clone(),
        text_payload("[slow] [long] Rust SDK installed-crate acceptance."),
    );
    let idempotency_key = key();
    let submitted = bound(
        &client,
        key_ws
            .threads()
            .create(
                &request,
                ThreadsCreateOptions {
                    idempotency_key: idempotency_key.clone(),
                    ..Default::default()
                },
            )
            .await?,
    )?;
    let replay = bound(
        &client,
        key_ws
            .threads()
            .create(
                &request,
                ThreadsCreateOptions {
                    idempotency_key,
                    ..Default::default()
                },
            )
            .await?,
    )?;
    ensure(
        submitted.receipt.status.as_u16() == 201
            && replay.receipt.status.as_u16() == 200
            && submitted.receipt.data.thread.id == replay.receipt.data.thread.id,
        "Create/replay statuses",
    )?;
    ensure(
        submitted.receipt.data.thread.workspace_id == canonical.id
            && submitted
                .thread
                .resource()
                .get(Default::default())
                .await?
                .data
                .workspace_id
                == canonical.id,
        "Canonical workspace binding",
    )?;
    sse::verify(&service, &ca, &token, &submitted.receipt.data.thread.id).await?;
    wait(accepted(&submitted)?, m::RunStatus::Completed).await?;
    println!("Verified HTTPS: typed submission, 201/200 replay, canonical binding, Run readback");

    let imported_history: Vec<std::collections::HashMap<String, serde_json::Value>> =
        serde_json::from_value(serde_json::json!([
            {"kind": "request", "parts": [{"part_kind": "user-prompt", "content": "Earlier question."}]},
            {"kind": "response", "parts": [{"part_kind": "text", "content": "Earlier answer."}]}
        ]))?;
    let mut imported_request =
        m::NewThread::new(agent.clone(), text_payload("Continue after import."));
    imported_request.message_history = Some(imported_history.clone());
    let imported = key_ws
        .threads()
        .create(
            &imported_request,
            ThreadsCreateOptions {
                idempotency_key: key(),
                ..Default::default()
            },
        )
        .await?;
    ensure(
        imported.data.thread.message_history == imported_history,
        "Low-level imported history receipt",
    )?;
    let imported_readback = key_ws
        .threads()
        .at(&imported.data.thread.id)
        .get(Default::default())
        .await?;
    ensure(
        imported_readback.data.message_history == imported_history,
        "Low-level imported history readback",
    )?;
    let mut high = client
        .agent(&agent)
        .start_with(
            "Summarize the prior conversation.",
            key(),
            a13n::StartOptions {
                message_history: Some(imported_history.clone()),
                ..Default::default()
            },
        )
        .await?;
    ensure(
        high.receipt.data.thread.message_history == imported_history,
        "High-level imported history receipt",
    )?;
    ensure(
        *high.result().await?.status() == m::RunStatus::Completed,
        "High-level imported history Run",
    )?;
    let mut followup = client
        .agent(&agent)
        .send(&high.thread.id, "Follow up without reimport.", key())
        .await?;
    ensure(
        *followup.result().await?.status() == m::RunStatus::Completed,
        "High-level follow-up Run",
    )?;
    let after_followup = high.thread.resource().get(Default::default()).await?;
    ensure(
        after_followup.data.message_history == imported_history,
        "Follow-up must not reseed imported history",
    )?;
    println!(
        "Verified HTTPS: low/high imported model context, Thread readback, no follow-up reseed"
    );

    // The ordinary installed-crate journey uses the finite authored Interaction,
    // including natural stream termination and authoritative exact-Run readback.
    let mut execution = client
        .agent(&agent)
        .start("[slow] [long] Rust finite Interaction acceptance.", key())
        .await?;
    let mut frames = 0;
    while tokio::time::timeout(Duration::from_secs(100), execution.next())
        .await??
        .is_some()
    {
        frames += 1;
        ensure(
            frames < 10_000,
            "Finite Interaction exceeded bounded event count",
        )?;
    }
    let outcome = execution.result().await?;
    ensure(
        *outcome.status() == m::RunStatus::Completed,
        "Finite Interaction Run status",
    )?;
    let items = outcome.run.items().get(Default::default()).await?;
    ensure(
        items.data.run.id == outcome.run.id && items.data.complete,
        "Finite Interaction committed Run Items",
    )?;

    let source = bound(
        &client,
        ws.threads()
            .create(
                &m::NewThread::new(
                    agent.clone(),
                    text_payload("[interruptible] Wait for queued inbox."),
                ),
                ThreadsCreateOptions {
                    idempotency_key: key(),
                    ..Default::default()
                },
            )
            .await?,
    )?;
    let mut queued = client
        .agent(&agent)
        .send(&source.thread.id, "Queued.", key())
        .await?;
    ensure(
        queued.receipt.data.run.is_none(),
        "Queued entry should have no Run",
    )?;
    wait(accepted(&source)?, m::RunStatus::Completed).await?;
    let queued_result = tokio::time::timeout(Duration::from_secs(100), queued.result()).await??;
    ensure(
        *queued_result.status() == m::RunStatus::Completed,
        "Queued interaction did not observe its exact incorporating Run",
    )?;
    let consumed = client
        .entry(&queued.thread.id, &queued.receipt.data.entry.id)
        .resource()
        .get(Default::default())
        .await?;
    ensure(
        consumed.data.status == m::EntryStatus::Consumed
            && consumed.data.assigned_run_id.as_deref() == Some(queued_result.run.id.as_str()),
        "Queued entry incorporation",
    )?;
    let interrupted = bound(
        &client,
        ws.threads()
            .create(
                &m::NewThread::new(agent.clone(), text_payload("[interruptible] Interrupt me.")),
                ThreadsCreateOptions {
                    idempotency_key: key(),
                    ..Default::default()
                },
            )
            .await?,
    )?;
    accepted(&interrupted)?
        .resource()
        .interrupt(Default::default())
        .await?;
    wait(accepted(&interrupted)?, m::RunStatus::Cancelled).await?;
    let fork = m::Fork::new(agent.clone(), text_payload("Fork response."));
    let forked = bound(
        &client,
        accepted(&submitted)?
            .resource()
            .fork(
                &fork,
                RunForkOptions {
                    idempotency_key: key(),
                    ..Default::default()
                },
            )
            .await?,
    )?;
    ensure(
        forked.receipt.data.thread.id != submitted.receipt.data.thread.id,
        "Fork Thread identity",
    )?;
    wait(accepted(&forked)?, m::RunStatus::Completed).await?;
    let mut waiting = client
        .agent(required("A13N_CLIENT_TOOL_AGENT")?)
        .start("[client] Review local SDK scenario.", key())
        .await?;
    let waiting_outcome = waiting.result().await?;
    ensure(
        *waiting_outcome.status() == m::RunStatus::Waiting,
        "Waiting client-tool outcome",
    )?;
    let pending = waiting_outcome
        .pending()
        .ok_or("Waiting Run has no pending client action")?;
    ensure(
        pending.approvals.is_empty() && pending.calls.len() == 1,
        "Pending client-tool call count",
    )?;
    let mut request = m::Resume::new(Default::default(), Default::default());
    request.calls.insert(
        pending.calls[0].tool_call_id.clone(),
        m::CallResult::Returned(Box::new(m::Returned::new(
            m::returned::Status::Returned,
            Some(serde_json::json!({"decision": "approved"})),
        ))),
    );
    request.input = Some(Some(Box::new(text_payload(
        "Additional context on the reviewed task.",
    ))));
    let resumed = waiting_outcome.run.resume(&request, key()).await?;
    ensure(
        resumed.receipt.data.id != waiting_outcome.run.id,
        "Resume should create successor Run",
    )?;
    wait(&resumed.run, m::RunStatus::Completed).await?;
    println!(
        "Verified HTTPS: queued entry, interrupt, fork, client-tool resume and exact Run waits"
    );

    let models = resources
        .models()
        .list(ModelsListOptions::default())
        .await?;
    ensure(
        !models.data.items.is_empty(),
        "Organization model catalogue",
    )?;
    let mut thread_pages = ws.threads().pages(ThreadsListOptions {
        limit: Some(2),
        ..Default::default()
    });
    ensure(
        thread_pages.next().await?.is_some(),
        "Bounded workspace list pagination",
    )?;

    let content = "asset\n".repeat(50_000).into_bytes();
    let uploaded = ws
        .uploads()
        .create(
            UploadFile {
                name: "acceptance.bin".into(),
                content_type: "application/octet-stream".into(),
                body: content.clone().into(),
            },
            UploadsCreateOptions {
                idempotency_key: key(),
                ..Default::default()
            },
        )
        .await?;
    let asset = ws
        .assets()
        .create(
            &m::AssetCreate::new("Rust acceptance".into(), uploaded.data.upload_id),
            AssetsCreateOptions::default(),
        )
        .await?;
    let mut download = ws
        .assets()
        .at(asset.data.id)
        .content()
        .get(Default::default())
        .await?;
    let mut received = Vec::new();
    while let Some(bytes) = download.chunk().await? {
        received.extend_from_slice(&bytes);
    }
    download.close();
    ensure(
        received == content && received.len() == 300_000,
        "300KB binary upload/download",
    )?;
    let old = ws.agents().at(&agent).get(Default::default()).await?;
    let etag = old.etag().ok_or("Agent ETag absent")?.to_owned();
    let changed = ws
        .agents()
        .at(&agent)
        .update(
            &m::AgentUpdate {
                description: Some(Some("Rust acceptance".into())),
                ..Default::default()
            },
            AgentUpdateOptions {
                if_match: etag.clone(),
                ..Default::default()
            },
        )
        .await?;
    ensure(
        changed.etag() != Some(etag.as_str()),
        "CAS ETag should advance",
    )?;
    let stale = ws
        .agents()
        .at(&agent)
        .update(
            &m::AgentUpdate::default(),
            AgentUpdateOptions {
                if_match: etag,
                ..Default::default()
            },
        )
        .await
        .expect_err("Stale ETag must fail");
    ensure(
        matches!(stale, Error::Api(ref value) if value.status == 412),
        "CAS conflict status",
    )?;
    // Valid generated 1x1 RGBA PNG; exercise the typed raw-image MIME path as well.
    let png: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    ws.agents()
        .at(&agent)
        .avatar()
        .replace(
            png.to_vec().into(),
            AgentAvatarReplaceOptions {
                if_match: changed.etag().ok_or("Updated Agent ETag missing")?.into(),
                content_type: "image/png".into(),
                ..Default::default()
            },
        )
        .await?;
    let mut avatar = ws
        .agents()
        .at(&agent)
        .avatar()
        .get(Default::default())
        .await?;
    let mut image = Vec::new();
    while let Some(chunk) = avatar.chunk().await? {
        image.extend_from_slice(&chunk);
    }
    avatar.close();
    ensure(image == png, "PNG avatar upload/download")?;
    println!(
        "Verified HTTPS: 300KB binary, PNG avatar, typed catalogues and structured CAS conflict"
    );
    memory(&client, &ws, &agent).await?;
    client.close();
    Ok(())
}

async fn memory(client: &Client, ws: &ServiceResources<'_>, agent: &str) -> Result<()> {
    let provider = client
        .resources()
        .memory_providers()
        .at(required("A13N_MEMORY_PROVIDER")?);
    ensure(
        provider.test(Default::default()).await?.data.status == m::provider_test::Status::Succeeded,
        "Memory provider probe",
    )?;
    let memory = ws
        .memories()
        .create(
            &m::MemoryCreate::new("Rust acceptance".into()),
            Default::default(),
        )
        .await?;
    let mem = ws.memories().at(memory.data.id.clone());
    let path = "projects/计划 #1%.md";
    let file = mem.files().at(path);
    let original = mem
        .files()
        .create(
            &m::MemoryFileCreate::new("first".into(), path.into()),
            Default::default(),
        )
        .await?;
    ensure(
        file.get(Default::default()).await?.data.content == "first",
        "Encoded Unicode file path",
    )?;
    file.replace(
        &m::MemoryFileReplace::new("second".into()),
        MemoryFileReplaceOptions {
            if_match: original.etag().ok_or("File ETag missing")?.into(),
            ..Default::default()
        },
    )
    .await?;
    let mut pages = mem.revisions().pages(MemoryRevisionsListOptions {
        path: Some(path.into()),
        limit: Some(1),
        ..Default::default()
    });
    let mut revisions = Vec::new();
    while let Some(page) = pages.next().await? {
        revisions.extend(page.data.items);
    }
    ensure(revisions.len() == 2, "Numeric revision pagination")?;
    let original_seq = revisions
        .iter()
        .find(|r| r.op == m::memory_revision::Op::Create)
        .ok_or("Create revision missing")?
        .seq;
    let updated_seq = revisions
        .iter()
        .find(|r| r.op == m::memory_revision::Op::Update)
        .ok_or("Update revision missing")?
        .seq;
    file.delete(MemoryFileDeleteOptions {
        if_match: file
            .get(Default::default())
            .await?
            .etag()
            .ok_or("File ETag absent")?
            .into(),
        ..Default::default()
    })
    .await?;
    let restore = mem
        .revisions()
        .at(updated_seq)
        .restore(MemoryRevisionRestoreOptions::default())
        .await?;
    ensure(
        restore
            .data
            .file
            .as_ref()
            .is_some_and(|file| file.content == "first"),
        "Restore absent target",
    )?;
    let restore = mem
        .revisions()
        .at(original_seq)
        .restore(MemoryRevisionRestoreOptions {
            if_match: Some(
                file.get(Default::default())
                    .await?
                    .etag()
                    .ok_or("File ETag absent")?
                    .into(),
            ),
            ..Default::default()
        })
        .await?;
    ensure(restore.data.file.is_none(), "Restore explicit null file")?;
    mem.revisions()
        .at(updated_seq)
        .restore(MemoryRevisionRestoreOptions::default())
        .await?;
    let mut request = m::NewThread::new(agent.into(), text_payload("Memory snapshot."));
    request.memories = Some(vec![m::MemoryMount::new(
        m::MemoryAccess::Read,
        memory.data.id,
        "notes".into(),
    )]);
    let submitted = bound(
        client,
        ws.threads()
            .create(
                &request,
                ThreadsCreateOptions {
                    idempotency_key: key(),
                    ..Default::default()
                },
            )
            .await?,
    )?;
    wait(accepted(&submitted)?, m::RunStatus::Completed).await?;
    let mount = submitted.thread.resource().get(Default::default()).await?;
    submitted
        .thread
        .resource()
        .memories()
        .at("notes")
        .update(
            &m::MemoryMountUpdate {
                access: Some(Some(m::MemoryAccess::Write)),
                ..Default::default()
            },
            ThreadMemoryUpdateOptions {
                if_match: mount.etag().ok_or("Thread ETag missing")?.into(),
                ..Default::default()
            },
        )
        .await?;
    ensure(
        accepted(&submitted)?
            .resource()
            .get(RunGetOptions::default())
            .await?
            .data
            .memory_mounts[0]
            .access
            == m::MemoryAccess::Read,
        "Frozen Run mounts",
    )?;
    let mut record_memory = m::MemoryCreate::new("Rust records".into());
    record_memory.r#type = Some("mem0_oss".into());
    record_memory.provider_id = Some(Some(required("A13N_MEMORY_PROVIDER")?));
    let record_memory = ws
        .memories()
        .create(&record_memory, Default::default())
        .await?;
    let records = ws.memories().at(record_memory.data.id).records();
    let record = records
        .create(
            &m::MemoryRecordText::new("prefers tea".into()),
            Default::default(),
        )
        .await?;
    records
        .at(&record.data.id)
        .replace(
            &m::MemoryRecordText::new("prefers coffee".into()),
            Default::default(),
        )
        .await?;
    let mut search = m::MemoryRecordSearch::new("coffee".into());
    search.limit = Some(3);
    let found = records.search(&search, Default::default()).await?;
    ensure(
        found.data.items.len() == 1 && found.data.items[0].id == record.data.id,
        "Record search",
    )?;
    records
        .at(&record.data.id)
        .delete(Default::default())
        .await?;
    ensure(
        records
            .list(MemoryRecordsListOptions::default())
            .await?
            .data
            .items
            .is_empty(),
        "Record delete",
    )?;
    println!(
        "Verified HTTPS: provider, Unicode memory/CAS, numeric history, nullable restore, frozen mounts, records CRUD/search"
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    offline().await?;
    if env::args().any(|a| a == "--offline") {
        return Ok(());
    }
    tokio::time::timeout(Duration::from_secs(250), live()).await??;
    Ok(())
}
