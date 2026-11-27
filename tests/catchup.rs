//! Behavioral coverage for the current Service snapshot's new native shapes.
mod common;
use a13n::{
    IsolatedSkillSync, IsolatedSkillSyncMode, OverrideSkillSync, Overrides, SharedSkillSync,
    SharedSkillSyncMode,
    generated::models,
    resources::SessionMessageAuthorsListOptions,
    streaming::{ItemRef, StreamOptions, ThreadFrame, ThreadStream},
};
use common::{Reply, client, sample, server};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

fn roundtrip<T: DeserializeOwned + Serialize>(value: Value) {
    let parsed: T = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), value);
}

#[test]
fn all_new_native_schema_shapes_roundtrip() {
    macro_rules! check {
        ($($name:ident),+ $(,)?) => { $(roundtrip::<models::$name>(sample(stringify!($name)));)+ };
    }
    check!(
        Analysis,
        AnalysisCreate,
        AnalysisPage,
        Assessment,
        Category,
        ContentRef,
        Evidence,
        FileSource,
        Finding,
        FindingCreate,
        FindingPage,
        FindingUpdate,
        IsolatedSkillSync,
        MemoryRecordProvenance,
        MemoryRecordSource,
        MemorySource,
        MessageAuthor,
        MessageAuthors,
        Preset,
        ProviderModel,
        RecordSource,
        RunContent,
        SelectedTrace,
        Severity,
        SharedSkillSync
    );
    for value in [
        Value::Null,
        json!(false),
        json!(0),
        json!([]),
        json!({"z":null,"a":[false,0]}),
    ] {
        roundtrip::<models::RunContent>(
            json!({"id":"content","media_type":"application/json","value":value,"truncated":false}),
        );
    }
    roundtrip::<models::RunContent>(
        json!({"id":"content","media_type":"text/plain","value":"{\"incomplete\":","truncated":true}),
    );
    assert!(
        serde_json::from_value::<models::RunContent>(
            json!({"id":"content","media_type":"text/plain"})
        )
        .is_err()
    );
    let source = json!({"principal_id":null,"run_id":null,"attempt_id":null,"tool_call_id":null,"confirmed_at":"2026-10-13T00:00:00Z"});
    roundtrip::<models::MemoryRecordProvenance>(
        json!({"created":null,"last_write":source,"content_matches":false}),
    );
}

#[test]
fn skill_policies_and_lazy_preparation_preserve_presence_and_explicit_discriminators() {
    assert_eq!(serde_json::to_value(Overrides::new()).unwrap(), json!({}));
    for mode in ["on_demand", "warmup", "eager"] {
        assert_eq!(
            serde_json::to_value(
                Overrides::new().lazy_environment_mode(Some(models::LazyEnvironment1::AnyOf0(
                    mode.to_owned()
                ),))
            )
            .unwrap(),
            json!({"lazy_environment": mode})
        );
    }
    assert_eq!(
        serde_json::to_value(Overrides::new().lazy_environment(None).skill_sync(None)).unwrap(),
        json!({"lazy_environment":null,"skill_sync":null})
    );
    let shared = OverrideSkillSync::Shared(Box::new(SharedSkillSync {
        mode: Some(SharedSkillSyncMode::Shared),
        overwrite: Some(false),
        cleanup: Some(false),
    }));
    let isolated = OverrideSkillSync::Isolated(Box::new(IsolatedSkillSync {
        mode: Some(IsolatedSkillSyncMode::Isolated),
        cache_count: Some(0),
    }));
    for (policy, expected) in [
        (
            shared,
            json!({"mode":"shared","overwrite":false,"cleanup":false}),
        ),
        (isolated, json!({"mode":"isolated","cache_count":0})),
    ] {
        assert_eq!(
            serde_json::to_value(
                Overrides::new()
                    .lazy_environment(Some(false))
                    .skill_sync(Some(policy))
            )
            .unwrap(),
            json!({"lazy_environment":false,"skill_sync":expected})
        );
        roundtrip::<models::SkillSync>(expected.clone());
        roundtrip::<models::AgentOverrideInputSkillSync>(expected);
    }
    for invalid in [
        json!({}),
        json!({"overwrite":false}),
        json!({"mode":null}),
        json!({"mode":"future"}),
    ] {
        assert!(serde_json::from_value::<models::SkillSync>(invalid.clone()).is_err());
        assert!(serde_json::from_value::<models::AgentOverrideInputSkillSync>(invalid).is_err());
    }
}

#[test]
fn memory_evidence_omitted_null_empty_and_malformed_are_distinct() {
    for memory_sources in [None, Some(Value::Null), Some(json!([]))] {
        let mut value = json!({"id":"item","kind":"text_message","state":"completed"});
        if let Some(sources) = memory_sources {
            value["memory_sources"] = sources;
        }
        roundtrip::<ItemRef>(value);
    }
    for invalid in [
        json!("bad"),
        json!([{}]),
        json!([{"kind":"file","memory":"notes","status":"future"}]),
    ] {
        assert!(serde_json::from_value::<ItemRef>(json!({"id":"item","kind":"text_message","state":"completed","memory_sources":invalid})).is_err());
    }
}

#[tokio::test]
async fn explicit_content_and_authors_reads_share_auth_and_do_not_expand_evidence() {
    let archived = json!({"id":"c /","media_type":"application/json","value":{"z":[false,null],"a":0},"truncated":false});
    let returned = archived.clone();
    let mut fixture = server(move |request| {
        if request.target.ends_with("/stream") {
            Reply::sse("event: delta\nid: 100-1\ndata: {\"run_id\":\"r\",\"attempt\":1,\"sequence\":1,\"event\":{\"type\":\"TEXT_MESSAGE_END\",\"messageId\":\"m\"},\"item\":{\"id\":\"m\",\"kind\":\"text_message\",\"state\":\"completed\",\"memory_sources\":[{\"kind\":\"record\",\"memory\":\"notes\",\"status\":\"recalled\",\"records\":[{\"id\":\"record\",\"sha256\":\"digest\"}]}]}}\n\n")
        } else if request.target.contains("/contents/") {
            Reply::json(200, returned.clone())
        } else {
            assert!(request.target.ends_with("/sessions/s/message-authors?entry_id=e&entry_id=e2"));
            Reply::json(200, json!({"items":[{"entry_id":"e","principal_id":"p","principal":null,"submitted_at":"2026-10-13T00:00:00Z"}]}))
        }
    }).await;
    let sdk = client(&fixture);
    let mut stream = ThreadStream::open(sdk.api().threads().at("t"), StreamOptions::default())
        .await
        .unwrap();
    let Some(ThreadFrame::Delta { data, .. }) = stream.next().await.unwrap() else {
        panic!("missing END")
    };
    assert_eq!(
        data.item.unwrap().memory_sources.unwrap().unwrap()[0].memory,
        "notes"
    );
    assert_eq!(fixture.requests.recv().await.unwrap().method, "GET");
    assert!(fixture.requests.try_recv().is_err()); // Evidence has no automatic expansion.
    let content = sdk
        .api()
        .runs()
        .at("r")
        .contents()
        .at("c /")
        .get(Default::default())
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&content.data).unwrap(), archived);
    assert_eq!(content.request_id(), Some("req_test"));
    let request = fixture.requests.recv().await.unwrap();
    assert!(request.target.ends_with("/runs/r/contents/c%20%2F"));
    assert!(request.headers.contains("authorization: bearer test-token"));
    let authors = sdk
        .api()
        .sessions()
        .at("s")
        .message_authors()
        .list(SessionMessageAuthorsListOptions {
            entry_id: vec!["e".into(), "e2".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(authors.data.items[0].entry_id, "e");
    let request = fixture.requests.recv().await.unwrap();
    assert!(request.headers.contains("authorization: bearer test-token"));
    sdk.close();
    assert!(matches!(
        sdk.api()
            .runs()
            .at("r")
            .contents()
            .at("c")
            .get(Default::default())
            .await,
        Err(a13n::Error::Closed)
    ));
    assert!(fixture.requests.try_recv().is_err());
}

#[tokio::test]
async fn logout_preserves_both_success_shapes_and_metadata() {
    use a13n::generated::apis::{auth_api, configuration::Configuration};
    for status in [200, 204] {
        let mut fixture = server(move |_| {
            if status == 200 {
                Reply::json(200, json!({"logout_url":"https://identity.example/logout"}))
            } else {
                Reply::bytes(204, "application/octet-stream", vec![])
            }
        })
        .await;
        let config = Configuration {
            base_path: fixture.url.clone(),
            ..Default::default()
        };
        let native = auth_api::logout(&config).await.unwrap();
        assert_eq!(native.status.as_u16(), status);
        assert_eq!(native.headers["x-request-id"], "req_test");
        assert_eq!(
            native.data.as_ref().map(|v| v.logout_url.as_str()),
            (status == 200).then_some("https://identity.example/logout")
        );
        let sdk = client(&fixture);
        let resource = sdk.api().auth().logout().await.unwrap();
        assert_eq!(resource.status.as_u16(), status);
        assert_eq!(resource.headers["x-request-id"], "req_test");
        assert_eq!(
            resource.data.as_ref().map(|v| v.logout_url.as_str()),
            (status == 200).then_some("https://identity.example/logout")
        );
        assert_eq!(
            fixture.requests.recv().await.unwrap().target,
            "/prefix/api/v1/auth/logout"
        );
        assert_eq!(
            fixture.requests.recv().await.unwrap().target,
            "/prefix/api/v1/auth/logout"
        );
        assert!(fixture.requests.try_recv().is_err());
    }
}

#[tokio::test]
async fn logout_rejects_null_json_for_object_success() {
    use a13n::generated::apis::{auth_api, configuration::Configuration};
    let fixture = server(|_| Reply::json(200, Value::Null)).await;
    let config = Configuration {
        base_path: fixture.url.clone(),
        ..Default::default()
    };
    assert!(auth_api::logout(&config).await.is_err());
    assert!(matches!(
        client(&fixture).api().auth().logout().await,
        Err(a13n::Error::Protocol(_))
    ));
}

#[tokio::test]
async fn generated_app_filter_query_is_unquoted_and_omittable() {
    use a13n::generated::apis::{browsers_api, configuration::Configuration};
    let mut fixture = server(|_| Reply::json(200, sample("BrowserPage"))).await;
    let config = Configuration {
        base_path: fixture.url.clone(),
        ..Default::default()
    };
    browsers_api::list_browsers(&config, None, None, Some("demo.app"), None)
        .await
        .unwrap();
    browsers_api::list_browsers(&config, None, None, None, None)
        .await
        .unwrap();
    let first = fixture.requests.recv().await.unwrap();
    let url = reqwest::Url::parse(&format!("http://localhost{}", first.target)).unwrap();
    assert_eq!(
        url.query_pairs()
            .find(|(key, _)| key == "app_id")
            .unwrap()
            .1,
        "demo.app"
    );
    let omitted = fixture.requests.recv().await.unwrap();
    let url = reqwest::Url::parse(&format!("http://localhost{}", omitted.target)).unwrap();
    assert!(!url.query_pairs().any(|(key, _)| key == "app_id"));
}
