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
