//! New snapshot journeys consuming only the installed crate's public API.
use super::{Result, ensure, key};
use a13n::{Client, Overrides, RunOptions, StartOptions, generated::models as m, resources::*};
use serde_json::{Value, json};
use std::collections::HashSet;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const PNG: &[u8] = &[137, 80, 78, 71, 13, 10, 26, 10, 0, 255, 128];

pub async fn offline() -> Result<()> {
    let mut space = serde_json::to_value(m::MemorySpace::default())?;
    space["id"] = json!("space");
    space["enabled"] = json!(false);
    space["always_load"] = json!([]);
    let mut session = serde_json::to_value(m::SessionView::default())?;
    session["memory"] = json!({"enabled":false,"spaces":[]});
    let mut run = serde_json::to_value(m::RunView::default())?;
    run["id"] = json!("r");
    run["thread_id"] = json!("t");
    let mounts = json!([
        {"name":"legacy","memory_id":"m","access":"read","recall":false},
        {"name":"shared","memory_id":"m","access":"write","roots":["global/","sessions/s/"],"space_id":"space"}
    ]);
    run["memory_mounts"] = mounts.clone();
    let mut mount = serde_json::to_value(m::MountView::default())?;
    mount["permission_ceiling"] = json!({"operations":[]});
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let client = Client::new(
        &format!("http://{}", listener.local_addr()?),
        a13n::Secret::new("offline-token"),
    )?;
    let server = tokio::spawn(async move {
        let cases = [
            ("POST /api/v1/memory-spaces ", 201, space),
            (
                "POST /api/v1/memory-spaces/space/organizations ",
                202,
                json!({"pending":true,"processed_seq":0,"history_gap":false}),
            ),
            ("PATCH /api/v1/sessions/s ", 200, session),
            ("GET /api/v1/runs/r?view=detailed ", 200, run),
            (
                "GET /api/v1/runs/r/agent ",
                200,
                serde_json::to_value(m::RunAgent::default()).unwrap(),
            ),
            (
                "GET /api/v1/runs/r/attempts/attempt ",
                200,
                serde_json::to_value(m::AttemptView::default()).unwrap(),
            ),
            (
                "GET /api/v1/usage/records/record/metrics ",
                200,
                json!({"time_to_first_chunk_seconds":null,"request_duration_seconds":0.0,"output_tokens_per_second":null}),
            ),
            (
                "POST /api/v1/environments/env%20%2F/computer/check ",
                200,
                json!({"status":"not_ready","display_count":0,"observe_ready":false,"input_ready":false,"operations":[]}),
            ),
            ("GET /api/v1/runs/r/images/image%20%2F ", 200, Value::Null),
            ("PATCH /api/v1/threads/t/environments/work ", 200, mount),
        ];
        for (expected, status, response) in cases {
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
            assert!(
                request.starts_with(expected),
                "Unexpected installed request: {}",
                request.lines().next().unwrap()
            );
            let (headers, body) = request.split_once("\r\n\r\n").unwrap();
            let headers = headers.to_ascii_lowercase();
            assert!(headers.contains("authorization: bearer offline-token"));
            if expected == "POST /api/v1/memory-spaces " {
                assert_eq!(
                    serde_json::from_str::<Value>(body).unwrap(),
                    json!({"name":"shared","enabled":false,"always_load":[],"organization":{"enabled":false,"cooldown_seconds":0,"trigger":{"type":"any_change"}}})
                );
            } else if expected.starts_with("PATCH /api/v1/sessions") {
                assert!(headers.contains("if-match: \"session:2\""));
                assert_eq!(
                    serde_json::from_str::<Value>(body).unwrap(),
                    json!({"memory":{"enabled":false,"spaces":[]}})
                );
            } else if expected.starts_with("PATCH") {
                assert!(headers.contains("if-match: \"thread:2\""));
                assert_eq!(
                    serde_json::from_str::<Value>(body).unwrap(),
                    json!({"permission_ceiling":{"operations":[]}})
                );
            }
            let binary = expected.contains("/images/");
            let encoded = if binary {
                PNG.to_vec()
            } else {
                serde_json::to_vec(&response).unwrap()
            };
            let media = if binary {
                "image/png"
            } else {
                "application/json"
            };
            socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Type: {media}\r\nETag: \"offline:1\"\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", encoded.len()).as_bytes()).await.unwrap();
            socket.write_all(&encoded).await.unwrap();
        }
    });
    let api = client.api();
    let created = api.memory_spaces().create(&serde_json::from_value(json!({"name":"shared","enabled":false,"always_load":[],"organization":{"enabled":false,"cooldown_seconds":0,"trigger":{"type":"any_change"}}}))?, Default::default()).await?;
    ensure(
        created.status.as_u16() == 201
            && !created.data.enabled
            && created.data.always_load.is_empty(),
        "Installed Space fields/status",
    )?;
    let requested = api
        .memory_spaces()
        .at("space")
        .organizations()
        .create(Default::default())
        .await?;
    ensure(
        requested.status.as_u16() == 202 && requested.data.pending == Some(true),
        "Installed organization request is status, not completed Run",
    )?;
    let changed = api
        .sessions()
        .at("s")
        .update(
            &serde_json::from_value(json!({"memory":{"enabled":false,"spaces":[]}}))?,
            SessionUpdateOptions {
                if_match: "\"session:2\"".into(),
                ..Default::default()
            },
        )
        .await?;
    ensure(
        changed.data.memory.unwrap().enabled == Some(false),
        "Installed Session memory",
    )?;
    let read = client.run("r").get().await?;
    ensure(
        serde_json::to_value(read.data.memory_mounts)? == mounts,
        "Installed legacy/frozen roots loss",
    )?;
    api.runs().at("r").agent().get(Default::default()).await?;
    api.runs()
        .at("r")
        .attempts()
        .at("attempt")
        .get(Default::default())
        .await?;
    let metrics = api
        .usage()
        .records()
        .at("record")
        .metrics()
        .get(Default::default())
        .await?;
    ensure(
        metrics.data.request_duration_seconds == Some(0.0)
            && metrics.data.time_to_first_chunk_seconds.is_none(),
        "Installed nullable/zero metrics",
    )?;
    let check = api
        .environments()
        .at("env /")
        .computer()
        .check(Default::default())
        .await?;
    ensure(
        check.data.status == m::computer_check::Status::NotReady,
        "Installed explicit readiness probe",
    )?;
    let mut image = api
        .runs()
        .at("r")
        .images()
        .at("image /")
        .get(Default::default())
        .await?;
    ensure(
        image.headers["content-type"] == "image/png"
            && image.headers["cache-control"] == "no-store",
        "Installed retained image metadata",
    )?;
    let mut bytes = Vec::new();
    while let Some(chunk) = image.chunk().await? {
        bytes.extend_from_slice(&chunk);
    }
    ensure(bytes == PNG, "Installed retained PNG bytes")?;
    api.threads()
        .at("t")
        .environments()
        .at("work")
        .update(
            &m::MountUpdate::new(m::EnvironmentPermissionSetInput {
                operations: Some(vec![]),
            }),
            ThreadEnvironmentUpdateOptions {
                if_match: "\"thread:2\"".into(),
                ..Default::default()
            },
        )
        .await?;
    server.await?;
    client.close();
    println!(
        "Installed crate local TCP: Space policy, Session memory/CAS, legacy/frozen roots, Run Agent/Attempt, nullable metrics, explicit Computer probe, retained PNG and mount ceiling passed (no cloud/provider execution)"
    );
    Ok(())
}

pub async fn live(client: &Client, agent: &str) -> Result<()> {
    let api = client.api();
    let created = api.memory_spaces().create(&m::MemorySpaceCreate { name: "Rust installed Space".into(), always_load: Some(vec![]), organization: Some(Box::new(serde_json::from_value(json!({"enabled":false,"cooldown_seconds":0,"trigger":{"type":"change_count","count":1}}))?)), ..Default::default() }, Default::default()).await?;
    ensure(created.status.as_u16() == 201, "Space creation HTTP status")?;
    let space = api.memory_spaces().at(&created.data.id);
    let current = space.get(Default::default()).await?;
    let changed = space
        .update(
            &m::MemorySpaceUpdate {
                description: Some(None),
                guide: Some(None),
                ..Default::default()
            },
            MemorySpaceUpdateOptions {
                if_match: current.etag().ok_or("Space ETag missing")?.into(),
                ..Default::default()
            },
        )
        .await?;
    ensure(
        changed.data.description.is_none() && changed.data.guide.is_none(),
        "Space nullable update",
    )?;
    ensure(
        api.memory_spaces()
            .list(Default::default())
            .await?
            .data
            .items
            .iter()
            .any(|s| s.id == created.data.id),
        "Space list",
    )?;
    let status = space.organization().get(Default::default()).await?;
    ensure(
        status.data.pending == Some(false) && status.data.active_run_id == Some(None),
        "Disabled organization status",
    )?;
    ensure(
        space
            .organizations()
            .list(Default::default())
            .await?
            .data
            .items
            .is_empty(),
        "New Space organization history",
    )?;
    let settings = m::SessionMemorySettings {
        enabled: Some(true),
        spaces: Some(vec![m::SpaceBinding::new(
            "shared".into(),
            created.data.id.clone(),
        )]),
    };
    let session = api
        .sessions()
        .create(
            &m::SessionCreate {
                memory: Some(Some(Box::new(settings))),
                ..Default::default()
            },
            Default::default(),
        )
        .await?;
    let mut interaction = client
        .agent(agent)
        .start_with(
            "Installed shared Space readback",
            StartOptions {
                session_id: Some(Some(session.data.id.clone())),
                idempotency_key: Some(key()),
                run_options: Some(
                    RunOptions::new()
                        .overrides(Some(Overrides::new().builtin_skills(Some(HashSet::new())))),
                ),
                ..Default::default()
            },
        )
        .await?;
    let outcome = interaction.result().await?;
    ensure(
        outcome.status == m::RunStatus::Completed,
        "Shared Space Run not completed",
    )?;
    let run = api.runs().at(&outcome.run_id);
    let read = client.run(&outcome.run_id).get().await?;
    let frozen = read
        .data
        .memory_mounts
        .iter()
        .find(|mount| {
            mount.space_id.as_ref().and_then(|id| id.as_deref()) == Some(created.data.id.as_str())
        })
        .ok_or("Frozen Space mount missing")?;
    ensure(
        frozen.roots.as_deref()
            == Some(
                &[
                    "global/".to_owned(),
                    format!("sessions/{}/", session.data.id),
                ][..],
            ),
        "Frozen Space roots",
    )?;
    let executed = run.agent().get(Default::default()).await?;
    let m::ResponseGetRunAgent::AnyOf0(executed) = executed.data else {
        return Err("Expected detailed executed Agent".into());
    };
    let config = serde_json::to_value(executed.config)?;
    ensure(
        config["builtin_skills"] == json!([]),
        "Executed Agent overrides",
    )?;
    ensure(
        api.runs()
            .list(RunsListOptions {
                session_id: Some(session.data.id.clone()),
                ..Default::default()
            })
            .await?
            .data
            .items
            .iter()
            .any(|r| r.id == outcome.run_id),
        "Filtered Run list",
    )?;
    let attempts = run.attempts().list(Default::default()).await?;
    let first = attempts.data.items.first().ok_or("Missing Run Attempt")?;
    ensure(
        run.attempts()
            .at(&first.id)
            .get(Default::default())
            .await?
            .data
            .run_id
            == outcome.run_id,
        "Attempt ownership",
    )?;
    let usage = api
        .usage()
        .get(UsageGetOptions {
            run_id: Some(outcome.run_id.clone()),
            ..Default::default()
        })
        .await?;
    ensure(
        usage.data.runs.iter().all(|r| r.run_id == outcome.run_id),
        "Run usage filter",
    )?;
    ensure(
        space
            .sessions()
            .list(Default::default())
            .await?
            .data
            .items
            .iter()
            .any(|s| s.id == session.data.id),
        "Attached Session list",
    )?;
    let selected = api
        .sessions()
        .at(&session.data.id)
        .get(Default::default())
        .await?;
    api.sessions()
        .at(&session.data.id)
        .update(
            &m::SessionUpdate {
                memory: Some(Some(Box::new(m::SessionMemorySettings {
                    enabled: Some(false),
                    spaces: Some(vec![]),
                }))),
                ..Default::default()
            },
            SessionUpdateOptions {
                if_match: selected.etag().ok_or("Session ETag missing")?.into(),
                ..Default::default()
            },
        )
        .await?;
    ensure(
        client.run(&outcome.run_id).get().await?.data.memory_mounts == read.data.memory_mounts,
        "Session edit changed accepted Run mount",
    )?;
    let current = space.get(Default::default()).await?;
    ensure(
        space
            .delete(MemorySpaceDeleteOptions {
                if_match: current.etag().ok_or("Space ETag missing")?.into(),
                ..Default::default()
            })
            .await?
            .status
            .as_u16()
            == 204,
        "Space deletion HTTP status",
    )?;
    if let Ok(model) = std::env::var("A13N_ORGANIZER_MODEL") {
        let organizer = api
            .memory_spaces()
            .create(
                &m::MemorySpaceCreate {
                    name: "Rust organization request".into(),
                    organization: Some(Box::new(serde_json::from_value(
                        json!({"enabled":false,"model":model}),
                    )?)),
                    ..Default::default()
                },
                Default::default(),
            )
            .await?;
        let requested = api
            .memory_spaces()
            .at(&organizer.data.id)
            .organizations()
            .create(Default::default())
            .await?;
        ensure(
            requested.status.as_u16() == 202,
            "Manual organization request status",
        )?;
        println!(
            "Verified HTTPS: configured manual organization request accepted 202 (execution not awaited; disposable Space retained for fixture cleanup)"
        );
    } else {
        println!(
            "NOT_RUN HTTPS manual organization request: A13N_ORGANIZER_MODEL not supplied; 202 wire contract covered offline"
        );
    }
    if let Ok(record) = std::env::var("A13N_MODEL_RECORD") {
        api.usage()
            .records()
            .at(record)
            .metrics()
            .get(Default::default())
            .await?;
        println!("Verified HTTPS: supplied model record metrics");
    } else {
        println!(
            "NOT_RUN HTTPS single-record metrics: A13N_MODEL_RECORD not supplied (nullable/zero wire coverage is offline)"
        );
    }
    println!(
        "Verified HTTPS: Space CRUD/CAS/status/history, attached Sessions, Session selection, frozen roots, Run search/executed Agent/Attempt and usage summary; Computer/provider/screenshots remain offline-only"
    );
    Ok(())
}
