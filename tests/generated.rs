mod common;
use a13n::{
    CallError, Secret,
    generated::{
        apis::{self, tenancy_api::*},
        models::*,
    },
};
use common::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
fn roundtrip<T: DeserializeOwned + Serialize>(value: Value) {
    let parsed: T = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), value);
}
#[test]
fn models_preserve_nullable_presence_unions_unknown_status_and_credentials() {
    for value in [
        json!({}),
        json!({"name":null}),
        json!({"name":"Name","labels":{"team":"dev"}}),
    ] {
        roundtrip::<AgentUpdate>(value);
    }
    let omitted: AgentUpdate = serde_json::from_value(json!({})).unwrap();
    let null: AgentUpdate = serde_json::from_value(json!({"name":null})).unwrap();
    assert_eq!(omitted.name, None);
    assert_eq!(null.name, Some(None));
    for value in [json!("completed"), json!("future_status")] {
        roundtrip::<RunStatus>(value);
    }
    for value in [
        json!({"type":"text","text":"hello"}),
        json!({"type":"json","value":{"arbitrary":[1,null,true]}}),
    ] {
        roundtrip::<Part>(value);
    }
    assert!(serde_json::from_value::<Part>(json!({"type":"future","value":1})).is_err());
    let secret:ProviderCreate=serde_json::from_value(json!({"name":"provider","type":"brave","workspace_id":null,"credential":{"api_key":"do-not-print"}})).unwrap();
    assert!(!format!("{secret:?}").contains("do-not-print"));
    assert_eq!(
        serde_json::to_value(secret).unwrap()["credential"]["api_key"],
        "do-not-print"
    );
    assert!(
        !format!(
            "{:?} {}",
            Secret::new("do-not-print"),
            Secret::new("do-not-print")
        )
        .contains("do-not-print")
    );
    roundtrip::<ToolSelection>(json!({"enabled":true,"permission":"inherit"}));
    assert!(serde_json::from_value::<ToolSelection>(json!({"enabled":"true"})).is_err());
    assert_eq!(
        serde_json::to_value(a13n::text_payload("hi")).unwrap(),
        json!({"content":[{"type":"text","text":"hi"}]})
    );
}
#[test]
fn model_price_rule_selectors_preserve_omission_null_and_values() {
    for value in [
        json!({"prices":[],"rule_id":"default"}),
        json!({"prices":[],"rule_id":"default","max_input_tokens":null,"service_tier":null}),
        json!({"prices":[],"rule_id":"default","max_input_tokens":128000,"service_tier":"priority"}),
    ] {
        roundtrip::<ModelPriceRuleInput>(value.clone());
        roundtrip::<ModelPriceRuleOutput>(value);
    }
    let mut input = ModelPriceRuleInput::new(vec![], "default".into());
    assert_eq!(input.max_input_tokens, None);
    assert_eq!(input.service_tier, None);
    input.max_input_tokens = Some(None);
    input.service_tier = Some(Some("priority".into()));
    assert_eq!(
        serde_json::to_value(input).unwrap(),
        json!({"prices":[],"rule_id":"default","max_input_tokens":null,"service_tier":"priority"})
    );
}

#[tokio::test]
async fn lowlevel_uses_owner_pool_prefix_path_encoding_and_metadata() {
    let mut server = server(|_| Reply::json(200, sample("Workspace"))).await;
    let client = client(&server);
    let result = client
        .execute(async |api| get_workspace_api_v1_workspaces_workspace_id_get(api, "id /+中").await)
        .await
        .unwrap();
    assert_eq!(result.status, 200);
    assert_eq!(result.etag(), Some("\"v1\""));
    assert_eq!(result.request_id(), Some("req_test"));
    let request = server.requests.recv().await.unwrap();
    assert_eq!(
        request.target,
        "/prefix/api/v1/workspaces/id%20%2F%2B%E4%B8%AD"
    );
    assert!(request.headers.contains("authorization: bearer test-token"));
    client.close();
    assert!(matches!(
        client
            .execute(async |api| get_workspace_api_v1_workspaces_workspace_id_get(api, "w").await)
            .await,
        Err(CallError::Closed)
    ));
}
#[tokio::test]
async fn lowlevel_image_mime_and_typed_error_remain_available() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-image.bin");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"image-body").unwrap();
    let mut server=server(|_|Reply::json(400,json!({"error":{"code":"invalid_argument","message":"Changed","details":{},"request_id":"req_test"}}))).await;
    let client = client(&server);
    let result = client
        .execute(async |api| {
            put_workspace_icon_api_v1_workspaces_workspace_id_icon_put(
                api,
                "w",
                "image/png",
                path.clone(),
                Some("\"v1\""),
            )
            .await
        })
        .await;
    let Err(CallError::Operation(apis::Error::ResponseError(response))) = result else {
        panic!("expected typed HTTP error")
    };
    assert_eq!(response.status, 400);
    assert_eq!(response.headers["x-request-id"], "req_test");
    assert!(matches!(
        response.entity,
        Some(PutWorkspaceIconApiV1WorkspacesWorkspaceIdIconPutError::Status400(_))
    ));
    let request = server.requests.recv().await.unwrap();
    assert!(request.headers.contains("content-type: image/png"));
    assert_eq!(request.body, b"image-body");
    std::fs::remove_file(path).unwrap();
}
#[tokio::test]
async fn lowlevel_sse_does_not_buffer_and_shares_shutdown_inside_execute() {
    let mut server = server(|_| {
        let mut reply = Reply::sse(": ping\n\n");
        reply
            .chunks
            .push((std::time::Duration::from_secs(30), vec![b'\n']));
        reply
    })
    .await;
    let client = std::sync::Arc::new(client(&server));
    let caller = client.clone();
    let task = tokio::spawn(async move {
        caller.execute(async |api| {let mut response=apis::runs_api::thread_stream_api_v1_threads_thread_id_stream_get(api,"t",Some("r"),Some("1-0"),None,None).await?;assert_eq!(response.status(),200);while response.chunk().await.map_err(apis::Error::from)?.is_some() {} Ok::<_,apis::Error<apis::runs_api::ThreadStreamApiV1ThreadsThreadIdStreamGetError>>(())}).await
    });
    let request = server.requests.recv().await.unwrap();
    assert_eq!(
        request.target,
        "/prefix/api/v1/threads/t/stream?run=r&position=1-0"
    );
    client.close();
    assert!(matches!(task.await.unwrap(), Err(CallError::Closed)));
}

#[test]
fn display_resume_hint_and_native_model_settings_preserve_wire_values() {
    for hint in [None, Some(Value::Null), Some(json!("100-2"))] {
        let mut value = sample("RunItems");
        value["position"] = json!("1-2");
        if let Some(hint) = hint {
            value["resume_after"] = hint;
        }
        roundtrip::<RunItems>(value);
    }
    for settings in [
        json!({}),
        json!({"provider_specific":{"enabled":true,"levels":[1,null,"auto"]},"timeout":2.5,"empty":null}),
    ] {
        let value =
            json!({"model_api":"provider:model","model_name":"example","settings":settings});
        roundtrip::<ModelConfigInput>(value.clone());
        roundtrip::<ModelConfigOutput>(value);
    }
    roundtrip::<AssetCreate>(
        json!({"name":"asset","upload_id":"upl_0123456789abcdef0123456789abcdef"}),
    );
    roundtrip::<BootstrapInput>(json!({"email":"test@example.org","password":"12345678"}));
    roundtrip::<PasswordChange>(json!({"current_password":"old","password":"12345678"}));
    roundtrip::<PasswordResetConfirm>(json!({"token":"token","password":"12345678"}));
}

#[tokio::test]
async fn generated_resources_forward_native_model_settings_and_upload_id() {
    let mut fixture = server(|_| {
        Reply::json(
            400,
            json!({"error":{"code":"invalid_argument","message":"fixture"}}),
        )
    })
    .await;
    let client = client(&fixture);
    let body: ModelCreate = serde_json::from_value(json!({"name":"model","provider_id":"provider","config":{"model_api":"provider:model","model_name":"example","settings":{"nested":{"array":[true,null,4]},"timeout":2.5}}})).unwrap();
    assert!(
        client
            .resources()
            .models()
            .create(&body, Default::default())
            .await
            .is_err()
    );
    let request = fixture.requests.recv().await.unwrap();
    assert_eq!(request.target, "/prefix/api/v1/models");
    let sent: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(
        sent["config"]["settings"],
        serde_json::to_value(&body).unwrap()["config"]["settings"]
    );
    let body = AssetCreate::new(
        "asset".into(),
        "upl_0123456789abcdef0123456789abcdef".into(),
    );
    assert!(
        client
            .resources()
            .assets()
            .create(&body, Default::default())
            .await
            .is_err()
    );
    let request = fixture.requests.recv().await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&request.body).unwrap()["upload_id"],
        body.upload_id
    );
}

#[test]
fn run_configuration_and_media_characteristics_preserve_omission_null_empty_and_native_values() {
    for value in [
        json!({}),
        json!({"configuration":null}),
        json!({"configuration":{}}),
        json!({"configuration":{"allowed_hosts":null}}),
        json!({"configuration":{"allowed_hosts":[],"extensions":{}}}),
        json!({"configuration":{"allowed_hosts":["UPPER.Example.","regex:.*\\.example"],"extensions":{"org.example":{"false":false,"zero":0,"empty":[],"nested":{"null":null}}}}}),
    ] {
        roundtrip::<RunOptionsInput>(value.clone());
        roundtrip::<RunOptionsOutput>(value);
    }
    for image in [
        None,
        Some(Value::Null),
        Some(json!({})),
        Some(
            json!({"split_large_images":false,"support_gif":false,"max_image_bytes":0,"max_image_dimension":0,"max_images":0}),
        ),
    ] {
        let mut value =
            json!({"url_input":{"video":["youtube"]},"video_input":{"max_video_bytes":4096}});
        if let Some(image) = image {
            value["image_input"] = image;
        }
        roundtrip::<HarnessModelCharacteristicsInput>(value.clone());
        roundtrip::<HarnessModelCharacteristicsOutput>(value);
    }
    roundtrip::<ProviderAuthorizationRequest>(json!({"new_registration":false}));
    roundtrip::<AuthorizationDisconnect>(
        json!({"local_tokens_cleared":true,"revocation_confirmed":null}),
    );
    let callback: AuthorizationCallback = serde_json::from_value(
        json!({"attempt_id":"attempt","callback_url":"https://callback.example/?code=secret&state=secret"}),
    )
    .unwrap();
    assert!(!format!("{callback:?}").contains("secret"));
}

#[tokio::test]
async fn native_model_media_policy_forwarding_never_applies_client_side_defaults() {
    let mut fixture = server(|_| {
        Reply::json(
            400,
            json!({"error":{"code":"invalid_argument","message":"capture"}}),
        )
    })
    .await;
    let sdk = client(&fixture);
    let body: ModelCreate = serde_json::from_value(json!({"name":"model","provider_id":"p","config":{"model_api":"provider:model","model_name":"m","characteristics":{"image_input":{"split_large_images":false,"max_image_bytes":0,"max_images":0},"url_input":{"video":["youtube"]},"video_input":{"max_video_bytes":4096}}}})).unwrap();
    sdk.resources()
        .models()
        .create(&body, Default::default())
        .await
        .unwrap_err();
    let request = fixture.requests.recv().await.unwrap();
    assert_eq!(request.json(), serde_json::to_value(body).unwrap());
}
