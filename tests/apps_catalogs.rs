mod common;
use a13n::{generated::models as m, resources::*};
use common::{Reply, client, sample, server};
use serde_json::{Value, json};

#[tokio::test]
async fn apps_preserve_explicit_requests_and_arbitrary_results() {
    let mut srv = server(|request| {
        let name = if request.target.ends_with("/open") {
            "OpenedApp"
        } else if request.target.ends_with("/read") {
            "AppResource"
        } else {
            "AppToolResult"
        };
        let mut value = sample(name);
        if name == "AppToolResult" {
            value["result"] = json!({"nested":[null,false,1,{"empty":[]}]});
        }
        Reply::json(200, value)
    })
    .await;
    let c = client(&srv);
    let app = c.api().runs().at("run").apps().at("app #%");
    app.open(Default::default()).await.unwrap();
    let request = srv.requests.recv().await.unwrap();
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.target,
        "/prefix/api/v1/runs/run/apps/app%20%23%25/open"
    );
    assert!(request.body.is_empty());
    app.resources()
        .read(
            &m::AppResourceRead::new("ui://chart".into()),
            Default::default(),
        )
        .await
        .unwrap();
    let request = srv.requests.recv().await.unwrap();
    assert!(request.target.ends_with("/resources/read"));
    assert_eq!(request.json(), json!({"uri":"ui://chart"}));
    let body = json!({"name":"chart","arguments":{"nested":[null,false,1]}});
    let result = app
        .tools()
        .call(
            &serde_json::from_value(body.clone()).unwrap(),
            Default::default(),
        )
        .await
        .unwrap();
    let request = srv.requests.recv().await.unwrap();
    assert!(request.target.ends_with("/tools/call"));
    assert_eq!(request.json(), body);
    assert_eq!(
        serde_json::to_value(result.data).unwrap(),
        json!({"result":{"nested":[null,false,1,{"empty":[]}]}})
    );
}

#[tokio::test]
async fn catalogs_archive_conditional_deletes_and_filtered_pages() {
    let mut srv = server(|request| {
        if request.method == "DELETE" {
            return Reply::bytes(204, "application/json", vec![]);
        }
        if request.target.contains("/content?") {
            return Reply::bytes(200, "application/zip", b"PK\0\xff".to_vec());
        }
        let name = if request.target.ends_with("/builtin-skills") {
            "BuiltinSkillCatalog"
        } else if request.target.ends_with("/platform-operations") {
            "BuiltinSkillDetail"
        } else if request.target.ends_with("/plugins") {
            "PluginCatalog"
        } else {
            return Reply::json(200, json!({"items":[],"next_cursor":null}));
        };
        Reply::json(200, sample(name))
    })
    .await;
    let c = client(&srv);
    c.api()
        .builtin_skills()
        .list(Default::default())
        .await
        .unwrap();
    c.api()
        .builtin_skills()
        .at("platform-operations")
        .get(Default::default())
        .await
        .unwrap();
    c.api().plugins().list(Default::default()).await.unwrap();
    for path in [
        "/builtin-skills",
        "/builtin-skills/platform-operations",
        "/plugins",
    ] {
        let request = srv.requests.recv().await.unwrap();
        assert_eq!(request.method, "GET");
        assert_eq!(request.target, format!("/prefix/api/v1{path}"));
    }
    let digest = "a".repeat(64);
    let mut archive = c
        .api()
        .builtin_skills()
        .at("platform-operations")
        .content()
        .get(BuiltinSkillContentGetOptions {
            package_digest: Some(digest.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut bytes = vec![];
    while let Some(chunk) = archive.chunk().await.unwrap() {
        bytes.extend_from_slice(&chunk);
    }
    assert_eq!(bytes, b"PK\0\xff");
    assert!(
        srv.requests
            .recv()
            .await
            .unwrap()
            .target
            .ends_with(&format!("?package_digest={digest}"))
    );
    let etag = "\"provider:2\"".to_string();
    c.api()
        .connector_providers()
        .at("p")
        .delete(ConnectorProviderDeleteOptions {
            if_match: etag.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    c.api()
        .memory_providers()
        .at("p")
        .delete(MemoryProviderDeleteOptions {
            if_match: etag.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    c.api()
        .model_providers()
        .at("p")
        .delete(ModelProviderDeleteOptions {
            if_match: etag.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    c.api()
        .web_providers()
        .at("p")
        .delete(WebProviderDeleteOptions {
            if_match: etag,
            ..Default::default()
        })
        .await
        .unwrap();
    for path in [
        "connector-providers",
        "memory-providers",
        "model-providers",
        "web-providers",
    ] {
        let request = srv.requests.recv().await.unwrap();
        assert_eq!(request.method, "DELETE");
        assert_eq!(request.target, format!("/prefix/api/v1/{path}/p"));
        assert!(request.headers.contains("if-match: \"provider:2\""));
    }
    let page = c
        .api()
        .sessions()
        .at("s")
        .threads()
        .list(SessionThreadsListOptions {
            origin: Some(vec!["new".into(), "child".into()]),
            thread_id: Some(vec!["t1".into(), "t2".into()]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(page.data.next_cursor.is_none());
    let target = srv.requests.recv().await.unwrap().target;
    assert!(target.contains("origin=new&origin=child"));
    assert!(target.contains("thread_id=t1&thread_id=t2"));
    c.api()
        .usage()
        .members()
        .list(UsageMembersListOptions {
            start: "2026-10-28T00:00:00Z".parse().unwrap(),
            end: "2026-10-29T00:00:00Z".parse().unwrap(),
            principal_id: Some("principal".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        srv.requests
            .recv()
            .await
            .unwrap()
            .target
            .contains("principal_id=principal")
    );
    let page = c
        .api()
        .usage()
        .runs()
        .list(UsageRunsListOptions {
            run_id: Some("r".into()),
            cursor: Some("next".into()),
            limit: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(page.data.next_cursor.is_none());
    let target = srv.requests.recv().await.unwrap().target;
    assert!(
        target.contains("run_id=r") && target.contains("cursor=next") && target.contains("limit=1")
    );
}

#[test]
fn new_models_preserve_nullable_required_fields_and_non_null_options() {
    let plugin = json!({"plugin_key":"p","display_name":"P","description":"","config_schema":null,"distribution_name":null,"distribution_version":null});
    let value: m::PluginType = serde_json::from_value(plugin.clone()).unwrap();
    assert_eq!(serde_json::to_value(value).unwrap(), plugin);
    for field in ["config_schema", "distribution_name", "distribution_version"] {
        let mut missing = plugin.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<m::PluginType>(missing).is_err());
    }
    for input in [
        json!({}),
        json!({"headers":{}}),
        json!({"headers":{"X-Tenant":"demo"}}),
    ] {
        let value: m::RequestContext = serde_json::from_value(input.clone()).unwrap();
        assert_eq!(serde_json::to_value(value).unwrap(), input);
    }
    assert!(serde_json::from_value::<m::RequestContext>(json!({"headers":null})).is_err());
    let policy: m::BackgroundCompactionPolicy =
        serde_json::from_value(json!({"enabled":false,"blocking_threshold":1.0})).unwrap();
    assert_eq!(
        serde_json::to_value(policy).unwrap(),
        json!({"enabled":false,"blocking_threshold":1.0})
    );
    let page: m::RunUsagePage =
        serde_json::from_value(json!({"items":[],"next_cursor":null})).unwrap();
    assert_eq!(
        serde_json::to_value(page).unwrap()["next_cursor"],
        Value::Null
    );
    assert!(serde_json::from_value::<m::RunUsagePage>(json!({"items":[]})).is_err());
}
