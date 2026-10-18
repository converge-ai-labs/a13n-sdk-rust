//! Semantic boundaries introduced by Service 647371c. Desktop execution is mocked.
mod common;
use a13n::{Overrides, generated::models as m, resources::*};
use common::{Reply, client, sample, server};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

fn roundtrip<T: DeserializeOwned + Serialize>(value: Value) {
    let typed: T = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), value);
}

#[test]
fn tool_search_native_proxy_and_override_presence_are_lossless() {
    for config in [
        json!({"mode":"proxy"}),
        json!({"mode":"proxy","search_name":"find","call_name":"call","max_results":7,"max_search_bytes":2048}),
        json!({"mode":"native"}),
        json!({"mode":"native","strategy":null}),
        json!({"mode":"native","strategy":"bm25"}),
        json!({"mode":"native","strategy":"regex"}),
    ] {
        roundtrip::<m::ToolSearchConfig>(config.clone());
        let tool_search = json!({"config":config,"groups":{}});
        roundtrip::<m::ToolSearch>(tool_search.clone());
        roundtrip::<m::AgentConfigInput>(json!({"model":"model","tool_search":tool_search}));
    }
    for value in [
        json!({}),
        json!({"tool_search":null}),
        json!({"tool_search":{}}),
        json!({"tool_search":{"groups":{}}}),
    ] {
        roundtrip::<m::AgentOverrideInput>(value.clone());
        roundtrip::<m::AgentOverrideOutput>(value);
    }
    assert_eq!(serde_json::to_value(Overrides::new()).unwrap(), json!({}));
    assert_eq!(
        serde_json::to_value(Overrides::new().tool_search(None)).unwrap(),
        json!({"tool_search":null})
    );
    let selected = m::ToolSearch {
        config: Some(Box::new(m::ToolSearchConfig::Native(Box::default()))),
        groups: Some(Default::default()),
    };
    assert_eq!(
        serde_json::to_value(Overrides::new().tool_search(Some(selected))).unwrap(),
        json!({"tool_search":{"config":{"mode":"native"},"groups":{}}})
    );
    roundtrip::<m::ToolSearch>(
        json!({"groups":{"work":{"description":"Selected sources","connections":[],"plugins":[],"platform":false}}}),
    );
}

#[test]
fn invalid_tool_search_union_is_rejected_at_typed_boundary() {
    for config in [
        json!({}),
        json!({"strategy":"regex"}),
        json!({"mode":null}),
        json!({"mode":"future"}),
        json!({"mode":"native","max_results":7}),
        json!({"mode":"proxy","strategy":"regex"}),
        json!({"mode":"native","strategy":"invalid"}),
        json!({"mode":"proxy","max_results":null}),
        json!({"mode":"proxy","search_name":null}),
        json!({"mode":"native","strategy":false}),
    ] {
        assert!(
            serde_json::from_value::<m::ToolSearchConfig>(config.clone()).is_err(),
            "{config}"
        );
        assert!(
            serde_json::from_value::<m::AgentConfigInput>(
                json!({"model":"m","tool_search":{"config":config}})
            )
            .is_err()
        );
    }
    for value in [
        json!({"config":null}),
        json!({"groups":null}),
        json!({"native_search":true}),
        json!({"groups":{"x":{"description":"x","platform":null}}}),
    ] {
        assert!(
            serde_json::from_value::<m::ToolSearch>(value.clone()).is_err(),
            "{value}"
        );
    }
}

#[test]
fn context_required_nullable_fields_preserve_null_and_zero() {
    for (input, output, window) in [
        (Value::Null, Value::Null, Value::Null),
        (json!(0), json!(7), Value::Null),
        (json!(4), json!(0), json!(128)),
    ] {
        let wire = json!({"run_id":"r","record_id":"record","input_tokens":input,"output_tokens":output,"context_window_tokens":window});
        roundtrip::<m::ContextUsage>(wire.clone());
        for field in [
            "run_id",
            "record_id",
            "input_tokens",
            "output_tokens",
            "context_window_tokens",
        ] {
            let mut missing = wire.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<m::ContextUsage>(missing).is_err(),
                "{field}"
            );
        }
    }
    for name in ["Control", "Completion", "Request"] {
        let mut wire = sample(name);
        if name != "Request" {
            wire["epoch"] = json!(0);
            wire["session_id"] = json!("browser_session_token_123456");
        }
        match name {
            "Control" => roundtrip::<m::Control>(wire),
            "Completion" => roundtrip::<m::Completion>(wire),
            _ => roundtrip::<m::Request>(wire),
        }
    }
}

#[tokio::test]
async fn context_and_desktop_absence_are_successful_null_not_defaults() {
    let mut fixture = server(|request| {
        if request.target.contains("run_id=present") {
            Reply::json(200, json!({"run_id":"present","record_id":"record","input_tokens":0,"output_tokens":null,"context_window_tokens":null}))
        } else if request.target.contains("run_id=missing") {
            Reply::json(404, json!({"error":{"code":"not_found","message":"Missing","details":{}}}))
        } else if request.target.ends_with("/usage/context") {
            Reply::json(400, json!({"error":{"code":"invalid_request","message":"Select exactly one","details":{}}}))
        } else { Reply::json(200, Value::Null) }
    }).await;
    let sdk = client(&fixture);
    let present = sdk
        .api()
        .usage()
        .context()
        .get(UsageContextGetOptions {
            run_id: Some("present".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(present.status, 200);
    let value = present.data.unwrap();
    assert_eq!(value.input_tokens, Some(0));
    assert_eq!(value.output_tokens, None);
    assert_eq!(value.context_window_tokens, None);
    let absent = sdk
        .api()
        .usage()
        .context()
        .get(UsageContextGetOptions {
            thread_id: Some("t /".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(absent.data.is_none());
    let desktop = sdk
        .api()
        .runs()
        .at("r /")
        .desktop_control()
        .get(Default::default())
        .await
        .unwrap();
    assert_eq!(desktop.status, 200);
    assert!(desktop.data.is_none());
    assert!(
        matches!(sdk.api().usage().context().get(Default::default()).await,Err(a13n::Error::Api(e)) if e.status==400)
    );
    assert!(
        matches!(sdk.api().usage().context().get(UsageContextGetOptions {run_id:Some("missing".into()),..Default::default()}).await,Err(a13n::Error::Api(e)) if e.status==404)
    );
    let mut targets = vec![];
    while let Ok(request) = fixture.requests.try_recv() {
        assert_eq!(request.method, "GET");
        targets.push(request.target);
    }
    assert_eq!(targets.len(), 5);
    assert!(targets[1].contains("thread_id=t+%2F"));
    assert!(targets[2].ends_with("/runs/r%20%2F/desktop-control"));
}

#[tokio::test]
async fn generated_native_nullable_responses_still_reject_invalid_json_and_nonnullable_null() {
    use a13n::generated::apis::{configuration::Configuration, usage_api};
    let fixture = server(|request| {
        if request.target.contains("record_id=empty") {
            Reply::bytes(200, "application/json", vec![])
        } else if request.target.contains("record_id=bad") {
            Reply::bytes(200, "application/json", b"{".to_vec())
        } else {
            Reply::json(200, Value::Null)
        }
    })
    .await;
    let config = Configuration {
        base_path: fixture.url.clone(),
        ..Default::default()
    };
    let response =
        usage_api::context_usage_api_v1_usage_context_get(&config, None, None, Some("none"), None)
            .await
            .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.headers["x-request-id"], "req_test");
    assert!(response.data.is_none());
    for record in ["empty", "bad"] {
        assert!(
            usage_api::context_usage_api_v1_usage_context_get(
                &config,
                None,
                None,
                Some(record),
                None
            )
            .await
            .is_err()
        );
    }
    let sdk = client(&fixture);
    for record in ["empty", "bad"] {
        assert!(matches!(
            sdk.api()
                .usage()
                .context()
                .get(UsageContextGetOptions {
                    record_id: Some(record.into()),
                    ..Default::default()
                })
                .await,
            Err(a13n::Error::Protocol(_))
        ));
    }
    assert!(
        sdk.api()
            .runs()
            .at("r")
            .get(Default::default())
            .await
            .is_err()
    );
}
