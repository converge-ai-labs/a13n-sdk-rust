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
