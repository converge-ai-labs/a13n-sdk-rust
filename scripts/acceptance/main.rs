//! Independent executable consuming only the extracted a13n .crate archive.
mod sse;
use a13n::{Client, Error, Secret, UploadFile, generated::models as m, resources::*, text_payload};
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
fn accepted<'a>(submitted: &'a a13n::Submitted<'_>) -> Result<&'a RunResource<'a>> {
    submitted
        .run
        .as_ref()
        .ok_or_else(|| "Accepted submission has no Run".into())
}
async fn wait(run: &RunResource<'_>, expected: m::RunStatus) -> Result<()> {
    let result = tokio::time::timeout(
        Duration::from_secs(80),
        run.wait(Duration::from_millis(100)),
    )
    .await??;
    ensure(
        result.data.status == expected,
        &format!("Unexpected Run status: {:?}", result.data.status),
    )?;
    let items = run.items().get().await?;
    ensure(
        items.data.run.id == result.data.id && items.data.complete,
        "Run Items mismatch",
    )
}

// Minimal offline TCP origin: enough to independently prove route, paging,
// HTTP evidence, structured error and close without a Service or fixture secret.
async fn offline() -> Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base_url = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut data = [0; 8192];
            let size = socket.read(&mut data).await.unwrap();
            let input = String::from_utf8_lossy(&data[..size]);
            assert!(
                input.contains("authorization: Bearer offline-token")
                    || input.contains("Authorization: Bearer offline-token")
            );
            let (status, body, extra) = if input
                .starts_with("GET /api/v1/workspaces/offline/threads")
            {
                (
                    "200 OK",
                    r#"{"items":[],"next_cursor":null}"#,
                    "ETag: \"offline\"\r\n",
                )
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
    let mut pages = resources
        .workspaces()
        .at("offline")
        .threads()
        .pages(ThreadsListOptions::default());
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
        "Installed crate local TCP: typed resource, metadata, pagination, 428, nullable and close passed"
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
    let ws = resources.workspaces().at(required("A13N_WORKSPACE")?);
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
    let canonical = ws.get().await?.data;
    let key_ws = resources.workspaces().at(canonical.key.clone());
    let request = m::NewThread::new(
        agent.clone(),
        text_payload("[slow] [long] Rust SDK installed-crate acceptance."),
    );
    let idempotency_key = key();
    let submitted = key_ws
        .threads()
        .create(
            &request,
            ThreadsCreateOptions {
                idempotency_key: idempotency_key.clone(),
            },
        )
        .await?;
    let replay = key_ws
        .threads()
        .create(&request, ThreadsCreateOptions { idempotency_key })
        .await?;
    ensure(
        submitted.receipt.status.as_u16() == 201
            && replay.receipt.status.as_u16() == 200
            && submitted.receipt.data.thread.id == replay.receipt.data.thread.id,
        "Create/replay statuses",
    )?;
    ensure(
        submitted.receipt.data.thread.workspace_id == canonical.id
            && submitted.thread.get().await?.data.workspace_id == canonical.id,
        "Canonical workspace binding",
    )?;
    sse::verify(
        &service,
        &ca,
        &token,
        &canonical.id,
        &submitted.receipt.data.thread.id,
    )
    .await?;
    wait(accepted(&submitted)?, m::RunStatus::Completed).await?;
    println!("Verified HTTPS: typed submission, 201/200 replay, canonical binding, Run readback");

    let source = ws
        .threads()
        .create(
            &m::NewThread::new(
                agent.clone(),
                text_payload("[interruptible] Wait for queued inbox."),
            ),
            ThreadsCreateOptions {
                idempotency_key: key(),
            },
        )
        .await?;
    let queued = source
        .thread
        .inbox()
        .create(
            &m::Message::new(agent.clone(), text_payload("Queued.")),
            InboxCreateOptions {
                idempotency_key: key(),
            },
        )
        .await?;
    ensure(queued.run.is_none(), "Queued entry should have no Run")?;
    wait(accepted(&source)?, m::RunStatus::Completed).await?;
    let consumed = tokio::time::timeout(
        Duration::from_secs(80),
        queued.entry.wait(Duration::from_millis(100)),
    )
    .await??;
    ensure(
        consumed.data.status == m::EntryStatus::Consumed,
        "Queued entry consumption",
    )?;
    let successor = ws.runs().at(consumed
        .data
        .assigned_run_id
        .ok_or("Queued entry missing assigned Run")?);
    wait(&successor, m::RunStatus::Completed).await?;
    let interrupted = ws
        .threads()
        .create(
            &m::NewThread::new(agent.clone(), text_payload("[interruptible] Interrupt me.")),
            ThreadsCreateOptions {
                idempotency_key: key(),
            },
        )
        .await?;
    accepted(&interrupted)?.interrupt().await?;
    wait(accepted(&interrupted)?, m::RunStatus::Cancelled).await?;
    let fork = m::Fork::new(agent.clone(), text_payload("Fork response."));
    let forked = accepted(&submitted)?
        .fork(
            &fork,
            RunForkOptions {
                idempotency_key: key(),
            },
        )
        .await?;
    ensure(
        forked.receipt.data.thread.id != submitted.receipt.data.thread.id,
        "Fork Thread identity",
    )?;
    wait(accepted(&forked)?, m::RunStatus::Completed).await?;
    let waiting = ws
        .threads()
        .create(
            &m::NewThread::new(
                required("A13N_CLIENT_TOOL_AGENT")?,
                text_payload("[client] Review local SDK scenario."),
            ),
            ThreadsCreateOptions {
                idempotency_key: key(),
            },
        )
        .await?;
    wait(accepted(&waiting)?, m::RunStatus::Waiting).await?;
    let pending = accepted(&waiting)?
        .get()
        .await?
        .data
        .pending
        .ok_or("Waiting Run has no pending client action")?;
    ensure(pending.items.len() == 1, "Pending client-tool action count")?;
    let answer = m::Answer::Complete(Box::new(m::Complete::new(
        m::complete::Action::Complete,
        Some(serde_json::json!({"decision": "approved"})),
        pending.items[0].tool_call_id.clone(),
    )));
    let mut request = m::ResumeRequest::new();
    request.answers = Some(vec![answer]);
    let resumed = accepted(&waiting)?
        .resume(
            &request,
            RunResumeOptions {
                idempotency_key: key(),
            },
        )
        .await?;
    ensure(
        resumed.data.id != accepted(&waiting)?.get().await?.data.id,
        "Resume should create successor Run",
    )?;
    wait(&ws.runs().at(resumed.data.id), m::RunStatus::Completed).await?;
    println!(
        "Verified HTTPS: queued entry, interrupt, fork, client-tool resume and exact Run waits"
    );

    let models = resources
        .organizations()
        .at(required("A13N_ORGANIZATION")?)
        .models()
        .list(OrganizationModelsListOptions::default())
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
            },
        )
        .await?;
    let asset = ws
        .assets()
        .create(&m::AssetCreate::new(
            "Rust acceptance".into(),
            uploaded.data.upload_id,
        ))
        .await?;
    let mut download = ws.assets().at(asset.data.id).content().get().await?;
    let mut received = Vec::new();
    while let Some(bytes) = download.chunk().await? {
        received.extend_from_slice(&bytes);
    }
    download.close();
    ensure(
        received == content && received.len() == 300_000,
        "300KB binary upload/download",
    )?;
    let old = ws.agents().at(&agent).get().await?;
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
            AgentUpdateOptions { if_match: etag },
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
            },
        )
        .await?;
    let mut avatar = ws.agents().at(&agent).avatar().get().await?;
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

async fn memory(client: &Client, ws: &WorkspaceResource<'_>, agent: &str) -> Result<()> {
    let provider = client
        .resources()
        .organizations()
        .at(required("A13N_ORGANIZATION")?)
        .memory_providers()
        .at(required("A13N_MEMORY_PROVIDER")?);
    ensure(
        provider.test().await?.data.status == m::provider_test::Status::Succeeded,
        "Memory provider probe",
    )?;
    let memory = ws
        .memories()
        .create(&m::MemoryCreate::new(key(), "Rust acceptance".into()))
        .await?;
    let mem = ws.memories().at(memory.data.id.clone());
    let path = "projects/计划 #1%.md";
    let file = mem.files().at(path);
    let original = mem
        .files()
        .create(&m::MemoryFileCreate::new("first".into(), path.into()))
        .await?;
    ensure(
        file.get().await?.data.content == "first",
        "Encoded Unicode file path",
    )?;
    file.replace(
        &m::MemoryFileReplace::new("second".into()),
        MemoryFileReplaceOptions {
            if_match: original.etag().ok_or("File ETag missing")?.into(),
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
        if_match: file.get().await?.etag().ok_or("File ETag absent")?.into(),
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
            if_match: Some(file.get().await?.etag().ok_or("File ETag absent")?.into()),
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
    let submitted = ws
        .threads()
        .create(
            &request,
            ThreadsCreateOptions {
                idempotency_key: key(),
            },
        )
        .await?;
    wait(accepted(&submitted)?, m::RunStatus::Completed).await?;
    let mount = submitted.thread.get().await?;
    submitted
        .thread
        .memories()
        .at("notes")
        .update(
            &m::MemoryMountUpdate {
                access: Some(Some(m::MemoryAccess::Write)),
                ..Default::default()
            },
            ThreadMemoryUpdateOptions {
                if_match: mount.etag().ok_or("Thread ETag missing")?.into(),
            },
        )
        .await?;
    ensure(
        accepted(&submitted)?.get().await?.data.memory_mounts[0].access == m::MemoryAccess::Read,
        "Frozen Run mounts",
    )?;
    let mut record_memory = m::MemoryCreate::new(key(), "Rust records".into());
    record_memory.r#type = Some("mem0_oss".into());
    record_memory.provider_id = Some(Some(required("A13N_MEMORY_PROVIDER")?));
    let record_memory = ws.memories().create(&record_memory).await?;
    let records = ws.memories().at(record_memory.data.id).records();
    let record = records
        .create(&m::MemoryRecordText::new("prefers tea".into()))
        .await?;
    records
        .at(&record.data.id)
        .replace(&m::MemoryRecordText::new("prefers coffee".into()))
        .await?;
    let mut search = m::MemoryRecordSearch::new("coffee".into());
    search.limit = Some(3);
    let found = records.search(&search).await?;
    ensure(
        found.data.items.len() == 1 && found.data.items[0].id == record.data.id,
        "Record search",
    )?;
    records.at(&record.data.id).delete().await?;
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
