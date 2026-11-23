//! Service contract wire regression; no live Service required.
mod common;
use a13n::{generated::models, resources::OrganizationAppPluginReplaceOptions};
use common::{Reply, client, server};
use serde_json::json;

#[tokio::test]
async fn app_plugins_get_put_preserves_false_and_if_match() {
    let mut fixture = server(|_| {
        Reply::json(
            200,
            json!({"id":"org_1","version":2,"permissions":[],"items":[]}),
        )
    })
    .await;
    let sdk = client(&fixture);
    let collection = sdk.api().organizations().at("org_1").app_plugins();
    let result = collection.list().await.unwrap();
    assert_eq!(result.data.version, 2);
    collection
        .at("app_1")
        .replace(
            &models::AppUpdate::new(false),
            OrganizationAppPluginReplaceOptions {
                if_match: "\"org_1:1\"".into(),
            },
        )
        .await
        .unwrap();
    let first = fixture.requests.recv().await.unwrap();
    let second = fixture.requests.recv().await.unwrap();
    assert_eq!(
        (first.method.as_str(), first.target.as_str()),
        ("GET", "/prefix/api/v1/organizations/org_1/app-plugins")
    );
    assert_eq!(
        (second.method.as_str(), second.target.as_str()),
        (
            "PUT",
            "/prefix/api/v1/organizations/org_1/app-plugins/app_1"
        )
    );
    assert!(
        second
            .headers
            .to_ascii_lowercase()
            .contains("if-match: \"org_1:1\"")
    );
    assert_eq!(second.json(), json!({"enabled":false}));
}

#[test]
fn cloudflare_create_uuid_serializes_as_a_string_and_redacts_secret() {
    let request_id = uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap();
    let request = models::DeploymentCreate::new(
        "0123456789abcdef0123456789abcdef".into(),
        "test-secret".into(),
        "example".into(),
        request_id,
    );
    let value = serde_json::to_value(&request).unwrap();
    assert_eq!(value["request_id"], "00000000-0000-4000-8000-000000000001");
    assert_eq!(value["api_token"], "test-secret");
    assert!(!format!("{request:?}").contains("test-secret"));
    let decoded: models::DeploymentCreate = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.request_id, request_id);
}

#[test]
fn cloudflare_deployment_cleanup_is_a_typed_map_on_the_wire() {
    let value = json!({
        "account_id": "0123456789abcdef0123456789abcdef",
        "cleanup": {"worker": "removed", "route": "pending"},
        "endpoint": null,
        "error": null,
        "id": "dep_one",
        "name": "example",
        "operation": "deploy",
        "provider_id": null,
        "source_image": "image",
        "stage": "complete",
        "status": "ready",
        "version": 3,
        "worker_name": "worker"
    });
    let deployment: models::Deployment = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(deployment.cleanup.len(), 2);
    assert_eq!(serde_json::to_value(&deployment).unwrap(), value);
    let mut invalid = value;
    invalid["cleanup"]["worker"] = json!("unexpected");
    assert!(serde_json::from_value::<models::Deployment>(invalid).is_err());
}

#[tokio::test]
async fn session_usage_selectors_and_typed_page_round_trip() {
    use a13n::resources::UsageSessionsListOptions;
    let mut fixture = server(|_| Reply::json(200, json!({
        "items": [{"session_id":"ses_123", "title":null,
            "usage":{"requests":2,"input_tokens":12,"output_tokens":4,"cache_read_tokens":0,"cache_hit_rate":null,"cost":null,"unpriced_requests":1},
            "runs":{"runs":3,"average_duration_seconds":null}}], "next_cursor":"later"
    }))).await;
    let sdk = client(&fixture);
    let response = sdk
        .api()
        .usage()
        .sessions()
        .list(UsageSessionsListOptions {
            start: "2026-01-01T00:00:00+00:00".parse().unwrap(),
            end: "2026-01-02T00:00:00+00:00".parse().unwrap(),
            agent_id: "agt_123".into(),
            limit: Some(7),
            cursor: Some("next +/".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(response.data.items[0].session_id, "ses_123");
    assert_eq!(response.data.items[0].usage.input_tokens, 12);
    assert_eq!(response.data.next_cursor.as_deref(), Some("later"));
    let request = fixture.requests.recv().await.unwrap();
    let url = url::Url::parse(&format!("https://example.test{}", request.target)).unwrap();
    let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(url.path(), "/prefix/api/v1/usage/sessions");
    assert_eq!(query.get("agent_id").map(String::as_str), Some("agt_123"));
    assert_eq!(query.get("cursor").map(String::as_str), Some("next +/"));
    assert_eq!(query.get("limit").map(String::as_str), Some("7"));
}

#[tokio::test]
async fn scoped_password_reset_and_trace_decimal_filters_preserve_wire_values() {
    use a13n::resources::TracesSearchListOptions;
    let mut fixture = server(|request| {
        if request.target.contains("password-reset") {
            Reply::json(200, json!({"email_queued":false,"reset_url":null}))
        } else {
            Reply::json(200, json!({"items":[],"next_cursor":null,"total":0,"collected_at":"2026-01-01T00:00:00Z"}))
        }
    }).await;
    let sdk = client(&fixture);
    let reset = sdk
        .api()
        .organizations()
        .at("org /1")
        .members()
        .at("user /2")
        .password_reset()
        .await
        .unwrap();
    assert!(!reset.data.email_queued);
    let timestamp = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap();
    let options = TracesSearchListOptions {
        started_after: timestamp,
        started_before: timestamp,
        min_duration_seconds: Some("0".into()),
        max_duration_seconds: Some("10.25".into()),
        min_cost_usd: Some("0.000000000000001".into()),
        max_cost_usd: Some("1".into()),
        ..Default::default()
    };
    sdk.api().traces().search().list(options).await.unwrap();
    let first = fixture.requests.recv().await.unwrap();
    let second = fixture.requests.recv().await.unwrap();
    assert_eq!(first.method, "POST");
    assert_eq!(
        first.target,
        "/prefix/api/v1/organizations/org%20%2F1/members/user%20%2F2/password-reset"
    );
    let url = reqwest::Url::parse(&format!("https://example.test{}", second.target)).unwrap();
    let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    for (key, expected) in [
        ("min_duration_seconds", "0"),
        ("max_duration_seconds", "10.25"),
        ("min_cost_usd", "0.000000000000001"),
        ("max_cost_usd", "1"),
    ] {
        assert_eq!(query.get(key).map(String::as_str), Some(expected));
    }
}
