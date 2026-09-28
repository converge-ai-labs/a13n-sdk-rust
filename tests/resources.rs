mod common;
use a13n::{generated::models, resources::*, *};
use common::*;
use serde_json::json;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
#[tokio::test]
async fn explicit_bindings_cas_headers_and_nullable_bodies() {
    let mut server = server(|_| Reply::json(200, sample("Agent"))).await;
    let client = client(&server);
    let resource = client.resources().agents().at("agent /+中");
    assert!(server.requests.try_recv().is_err());
    let body = models::AgentUpdate {
        name: Some(None),
        labels: Some(Some([("team".into(), "dev".into())].into())),
        ..Default::default()
    };
    assert!(matches!(
        resource.update(&body, AgentUpdateOptions::default()).await,
        Err(Error::InvalidInput)
    ));
    assert!(server.requests.try_recv().is_err());
    let response = resource
        .update(
            &body,
            AgentUpdateOptions {
                if_match: "\"v1\"".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(response.etag(), Some("\"v1\""));
    assert_eq!(response.request_id(), Some("req_test"));
    let request = server.requests.recv().await.unwrap();
    assert_eq!(request.method, "PATCH");
    assert_eq!(
        request.target,
        "/prefix/api/v1/agents/agent%20%2F+%E4%B8%AD"
    );
    assert!(request.headers.contains("if-match: \"v1\""));
    assert_eq!(request.json(), json!({"name":null,"labels":{"team":"dev"}}));
    for bad in ["", ".", ".."] {
        assert!(matches!(
            client.resources().workspaces().at(bad).get().await,
            Err(Error::InvalidInput)
        ));
    }
    client.close();
    assert!(matches!(
        resource.get(Default::default()).await,
        Err(Error::Closed)
    ));
}
#[tokio::test]
async fn paging_is_lazy_owned_repeated_query_and_loop_checked() {
    let mut server=server(|request|Reply::json(200,json!({"items":[],"next_cursor":if request.target.contains("cursor=later") {json!(null)}else{json!("later")}}))).await;
    let client = client(&server);
    let mut original = AgentsListOptions {
        label: Some(vec!["team:中文".into(), "enabled:true".into()]),
        q: Some("a b".into()),
        limit: Some(2),
        ..Default::default()
    };
    let mut pages = client.resources().agents().pages(original.clone());
    original.q = Some("changed".into());
    assert!(server.requests.try_recv().is_err());
    assert_eq!(pages.next().await.unwrap().unwrap().etag(), Some("\"v1\""));
    assert!(pages.next().await.unwrap().is_some());
    assert!(pages.next().await.unwrap().is_none());
    for cursor in [None, Some("later")] {
        let request = server.requests.recv().await.unwrap();
        let url = reqwest::Url::parse(&format!("http://localhost{}", request.target)).unwrap();
        let query: Vec<_> = url.query_pairs().collect();
        assert!(query.contains(&("q".into(), "a b".into())));
        assert_eq!(query.iter().filter(|(key, _)| key == "label").count(), 2);
        assert_eq!(
            query
                .iter()
                .find(|(key, _)| key == "cursor")
                .map(|(_, v)| v.as_ref()),
            cursor
        );
    }
    let looping = server_loop().await;
    let client = common::client(&looping);
    let mut pages = client.resources().workspaces().pages(Default::default());
    pages.next().await.unwrap();
    assert!(matches!(pages.next().await, Err(Error::Protocol(_))));
    assert!(pages.next().await.unwrap().is_none());
}
async fn server_loop() -> Server {
    server(|_| Reply::json(200, json!({"items":[],"next_cursor":"same"}))).await
}
#[tokio::test]
async fn structured_failures_bounds_no_retry_and_redirect_policy() {
    for status in [403, 409, 412, 429, 503] {
        let mut server=server(move |_|{let mut reply=Reply::json(status,json!({"error":{"code":"precondition_failed","message":"Changed","details":{"version":2},"request_id":"req_body"}}));reply.headers.push_str("Retry-After: 5\r\n");reply}).await;
        let client = client(&server);
        let Error::Api(error) = client
            .resources()
            .workspaces()
            .at("w")
            .get()
            .await
            .unwrap_err()
        else {
            panic!("api error")
        };
        assert_eq!(error.status, status);
        assert_eq!(error.request_id.as_deref(), Some("req_body"));
        assert_eq!(error.retry_after.as_deref(), Some("5"));
        assert_eq!(error.details["version"], 2);
        assert_eq!(error.headers["etag"], "\"v1\"");
        server.requests.recv().await.unwrap();
        assert!(server.requests.try_recv().is_err());
    }
    for body in [b"malformed".to_vec(), vec![b'x'; 1025]] {
        let server = server(move |_| Reply::bytes(200, "application/json", body.clone())).await;
        let client = Client::builder(&server.url)
            .response_limit(1024)
            .build()
            .unwrap();
        assert!(matches!(
            client.resources().workspaces().at("w").get().await,
            Err(Error::Protocol(_))
        ));
    }
    let mut server = server(|_| {
        let mut reply = Reply::json(200, json!({}));
        reply.disconnect = true;
        reply
    })
    .await;
    let client = client(&server);
    assert!(matches!(
        client
            .resources()
            .threads()
            .create(
                &models::NewThread::new("a".into(), text_payload("hi")),
                ThreadsCreateOptions {
                    idempotency_key: "same".into(),
                    ..Default::default()
                }
            )
            .await,
        Err(Error::Transport(_))
    ));
    server.requests.recv().await.unwrap();
    assert!(server.requests.try_recv().is_err());
    let mut callback = common::server(|_| {
        let mut reply = Reply::bytes(303, "text/plain", Vec::new());
        reply
            .headers
            .push_str("Location: https://example.invalid/finish\r\n");
        reply
    })
    .await;
    let client = common::client(&callback);
    let response = client
        .resources()
        .connections()
        .callback()
        .get(ConnectionsCallbackGetOptions {
            state: "state".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(response.status, 303);
    assert_eq!(
        response.headers["location"],
        "https://example.invalid/finish"
    );
    callback.requests.recv().await.unwrap();
    assert!(callback.requests.try_recv().is_err());
}
#[tokio::test]
async fn explicit_workspace_header_replaces_session_default_once_and_unscoped_stays_clean() {
    let mut server = server(|request| {
        if request.target.contains("/agents/") {
            Reply::json(200, sample("Agent"))
        } else {
            Reply::json(200, sample("Workspace"))
        }
    })
    .await;
    let client = Client::builder(&server.url)
        .workspace("workspace_A")
        .build()
        .unwrap();
    client
        .resources()
        .agents()
        .at("a")
        .get(AgentGetOptions {
            x_workspace_id: Some("workspace_B".into()),
        })
        .await
        .unwrap();
    client
        .resources()
        .agents()
        .at("a")
        .get(Default::default())
        .await
        .unwrap();
    client.resources().workspaces().at("w").get().await.unwrap();
    let scope = |headers: &str| -> Vec<String> {
        headers
            .lines()
            .filter(|line| line.starts_with("x-workspace-id:"))
            .map(str::to_owned)
            .collect()
    };
    let explicit = server.requests.recv().await.unwrap();
    assert_eq!(scope(&explicit.headers), ["x-workspace-id: workspace_b"]);
    let default = server.requests.recv().await.unwrap();
    assert_eq!(scope(&default.headers), ["x-workspace-id: workspace_a"]);
    let unscoped = server.requests.recv().await.unwrap();
    assert!(scope(&unscoped.headers).is_empty());
}

#[tokio::test]
async fn session_uses_current_csrf_and_shared_cookie_jar() {
    let mut server = server(|_| Reply::json(200, sample("Workspace"))).await;
    let jar = Arc::new(reqwest::cookie::Jar::default());
    jar.add_cookie_str(
        "session=valid; Path=/",
        &reqwest::Url::parse(&server.url).unwrap(),
    );
    let csrf = Arc::new(Mutex::new("first".to_string()));
    let current = csrf.clone();
    let client = Client::builder(&server.url)
        .session(jar.clone(), move || Some(current.lock().unwrap().clone()))
        .http_builder(reqwest::Client::builder().no_proxy())
        .build()
        .unwrap();
    let workspace = client.resources().workspaces().at("w");
    workspace.get().await.unwrap();
    let request = server.requests.recv().await.unwrap();
    assert!(request.headers.contains("cookie: session=valid"));
    assert!(!request.headers.contains("x-csrf-token"));
    assert!(!request.headers.contains("authorization"));
    for token in ["first", "second"] {
        *csrf.lock().unwrap() = token.into();
        workspace
            .update(
                &models::WorkspaceUpdate::default(),
                WorkspaceUpdateOptions {
                    if_match: "v1".into(),
                },
            )
            .await
            .unwrap();
        assert!(
            server
                .requests
                .recv()
                .await
                .unwrap()
                .headers
                .contains(&format!("x-csrf-token: {token}"))
        );
    }
    *csrf.lock().unwrap() = "advanced".into();
    client.execute(async |api|a13n::generated::apis::tenancy_api::update_workspace_api_v1_workspaces_workspace_id_patch(api,"w",models::WorkspaceUpdate::default(),Some("v1")).await).await.unwrap();
    assert!(
        server
            .requests
            .recv()
            .await
            .unwrap()
            .headers
            .contains("x-csrf-token: advanced")
    );
    assert!(
        Client::builder(&server.url)
            .bearer(Secret::new("token"))
            .session(jar, || None)
            .build()
            .is_err()
    );
}
#[tokio::test]
async fn streaming_download_and_owned_uploads_preserve_mime_and_shutdown() {
    let mut server = server(|request| {
        if request.target.ends_with("/uploads") {
            Reply::json(200, sample("Upload"))
        } else if request.method == "PUT" {
            Reply::json(200, sample("Workspace"))
        } else {
            let mut reply = Reply::bytes(200, "application/octet-stream", vec![1; 300000]);
            reply.chunks.push((Duration::from_secs(30), vec![2]));
            reply
        }
    })
    .await;
    let client = client(&server);
    let workspace = client.resources().workspaces().at("w");
    let file = UploadFile {
        name: "upload.bin".into(),
        content_type: "application/octet-stream".into(),
        body: vec![7; 300000].into(),
    };
    client
        .resources()
        .uploads()
        .create(
            file,
            UploadsCreateOptions {
                idempotency_key: "upload".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let request = server.requests.recv().await.unwrap();
    assert!(request.headers.contains("multipart/form-data"));
    assert!(
        request
            .body
            .windows(300000)
            .any(|b| b.iter().all(|&v| v == 7))
    );
    assert!(
        request
            .body
            .windows(b"filename=\"upload.bin\"".len())
            .any(|b| b == b"filename=\"upload.bin\"")
    );
    workspace
        .icon()
        .replace(
            vec![137, 80, 78, 71].into(),
            IconReplaceOptions {
                if_match: "v1".into(),
                content_type: "image/png".into(),
            },
        )
        .await
        .unwrap();
    let request = server.requests.recv().await.unwrap();
    assert!(request.headers.contains("content-type: image/png"));
    assert_eq!(request.body, [137, 80, 78, 71]);
    assert!(matches!(
        workspace
            .icon()
            .replace(
                vec![1].into(),
                IconReplaceOptions {
                    if_match: "v1".into(),
                    content_type: "text/plain".into()
                }
            )
            .await,
        Err(Error::InvalidInput)
    ));
    let mut response = tokio::time::timeout(
        Duration::from_secs(1),
        client
            .resources()
            .assets()
            .at("a")
            .content()
            .get(Default::default()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status, 200);
    let mut received = 0;
    while received < 300000 {
        received += response.chunk().await.unwrap().unwrap().len();
    }
    client.close();
    assert!(matches!(response.chunk().await, Err(Error::Closed)));
}
#[tokio::test]
async fn submitted_uses_canonical_scope_and_queued_entry_has_no_run() {
    let mut receipt = sample("Submitted");
    receipt["thread"]["workspace_id"] = json!("ws_canonical");
    receipt["thread"]["id"] = json!("thread_canonical");
    receipt["entry"]["id"] = json!("entry_canonical");
    receipt["entry"]["thread_id"] = json!("thread_canonical");
    receipt["run"] = serde_json::Value::Null;
    let mut server = server(move |request| {
        if request.method == "POST" {
            Reply::json(200, receipt.clone())
        } else {
            Reply::json(200, sample("ThreadView"))
        }
    })
    .await;
    let client = client(&server);
    let body = models::NewThread::new("agent".into(), text_payload("hi"));
    let raw = client
        .resources()
        .threads()
        .create(
            &body,
            ThreadsCreateOptions {
                idempotency_key: "request".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let submitted = Submitted::bind(&client, raw).unwrap();
    assert!(submitted.run.is_none());
    assert_eq!(submitted.receipt.status, 200);
    submitted
        .thread
        .resource()
        .get(Default::default())
        .await
        .unwrap();
    server.requests.recv().await.unwrap();
    assert_eq!(
        server.requests.recv().await.unwrap().target,
        "/prefix/api/v1/threads/thread_canonical"
    );
}
#[tokio::test]
async fn waits_use_exact_identity_and_cancel_requests_and_sleeps() {
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let mut server = server(move |request| {
        let n = calls.fetch_add(1, Ordering::SeqCst);
        if request.target.contains("/runs/") {
            let mut run = sample("RunView");
            run["id"] = json!("exact");
            run["status"] = json!(if n == 0 { "running" } else { "waiting" });
            Reply::json(200, run)
        } else {
            let mut entry = sample("EntryView");
            entry["id"] = json!("e");
            entry["thread_id"] = json!("t");
            entry["status"] = json!(if n == 2 { "assigned" } else { "consumed" });
            Reply::json(200, entry)
        }
    })
    .await;
    let client = client(&server);
    let result = client
        .run("exact")
        .wait_with(Duration::from_secs(1), Duration::from_millis(1))
        .await
        .unwrap();
    assert_eq!(result.snapshot.data.status, models::RunStatus::Waiting);
    let result = client
        .entry("t", "e")
        .wait(Duration::from_millis(1))
        .await
        .unwrap();
    assert_eq!(result.data.status, models::EntryStatus::Consumed);
    for _ in 0..2 {
        assert!(
            server
                .requests
                .recv()
                .await
                .unwrap()
                .target
                .ends_with("/runs/exact")
        );
    }
    for _ in 0..2 {
        assert!(
            server
                .requests
                .recv()
                .await
                .unwrap()
                .target
                .ends_with("/inbox/e")
        );
    }
    let mut hung = common::server(|_| {
        let mut reply = Reply::json(200, sample("RunView"));
        reply.delay = Duration::from_secs(30);
        reply
    })
    .await;
    let client = Arc::new(common::client(&hung));
    let caller = client.clone();
    let pending = tokio::spawn(async move { caller.run("r").wait().await.map(|_| ()) });
    hung.requests.recv().await.unwrap();
    client.close();
    assert!(matches!(pending.await.unwrap(), Err(Error::Closed)));
    let sleeping = common::server(|_| {
        let mut run = sample("RunView");
        run["id"] = json!("r");
        run["status"] = json!("running");
        Reply::json(200, run)
    })
    .await;
    let client = common::client(&sleeping);
    let run = client.run("r");
    assert!(
        tokio::time::timeout(Duration::from_millis(30), run.wait())
            .await
            .is_err()
    );
}
