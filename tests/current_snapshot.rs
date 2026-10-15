//! Semantic regressions for Service 74affc9f beyond generated operation coverage.
mod common;
use a13n::{
    BuiltinSkill, Overrides, PlatformGroup, PlatformReadPermission, PlatformSelection,
    PlatformWritePermission, generated::models as m, resources::*,
};
use common::{Reply, client, sample, server};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::collections::HashSet;

fn roundtrip<T: DeserializeOwned + Serialize>(value: Value) {
    let typed: T = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), value);
}

#[test]
fn added_native_schemas_preserve_their_wire_shapes() {
    macro_rules! check { ($($rust:ident => $schema:literal),+ $(,)?) => { $(roundtrip::<m::$rust>(sample($schema));)+ }; }
    check!(
        AnyChangeTrigger => "AnyChangeTrigger", ChangeCountTrigger => "ChangeCountTrigger",
        ComputerCheck => "ComputerCheck", EnvironmentAction => "EnvironmentAction",
        EnvironmentPermissionSetInput => "EnvironmentPermissionSet-Input",
        EnvironmentPermissionSetOutput => "EnvironmentPermissionSet-Output",
        EnvironmentProviderReferences => "EnvironmentProviderReferences",
        FrozenMemoryMount => "FrozenMemoryMount", MemorySpace => "MemorySpace",
        MemorySpaceCreate => "MemorySpaceCreate", MemorySpacePage => "MemorySpacePage",
        MemorySpaceUpdate => "MemorySpaceUpdate", ModelRequestMetrics => "ModelRequestMetrics",
        MountUpdate => "MountUpdate", OrganizationPolicy => "OrganizationPolicy",
        PlatformGroup => "PlatformGroup", PlatformSelection => "PlatformSelection",
        PresetKind => "PresetKind", ProviderReference => "ProviderReference",
        RunAgent => "RunAgent", RunUsage => "RunUsage",
        SessionMemorySettings => "SessionMemorySettings", SpaceBinding => "SpaceBinding",
        SpaceOrganizationStatus => "SpaceOrganizationStatus"
    );
    for status in ["checked", "unsupported", "not_ready"] {
        roundtrip::<m::ComputerCheck>(
            json!({"status":status,"display_count":0,"input_ready":false,"observe_ready":false,"operations":[]}),
        );
    }
    roundtrip::<m::ModelRequestMetrics>(
        json!({"time_to_first_chunk_seconds":null,"request_duration_seconds":0.0,"output_tokens_per_second":null}),
    );
    assert!(
        serde_json::from_value::<m::ModelRequestMetrics>(
            json!({"request_duration_seconds":0.0,"output_tokens_per_second":null})
        )
        .is_err()
    );
    for trigger in [
        json!({"type":"any_change"}),
        json!({"type":"change_count","count":1}),
    ] {
        roundtrip::<m::OrganizationPolicy>(
            json!({"trigger":trigger,"enabled":false,"cooldown_seconds":0,"check_interval_seconds":10,"model":null}),
        );
    }
    for trigger in [
        json!({}),
        json!({"type":"future"}),
        json!({"type":"change_count"}),
    ] {
        assert!(
            serde_json::from_value::<m::OrganizationPolicy>(json!({"trigger":trigger})).is_err()
        );
    }
}

#[test]
fn authored_platform_and_session_memory_keep_omitted_null_empty_and_false() {
    assert_eq!(serde_json::to_value(Overrides::new()).unwrap(), json!({}));
    assert_eq!(
        serde_json::to_value(Overrides::new().builtin_skills(None).platform_tools(None)).unwrap(),
        json!({"builtin_skills":null,"platform_tools":null})
    );
    let empty = Overrides::new()
        .builtin_skills(Some(HashSet::new()))
        .platform_tools(Some(PlatformSelection {
            groups: Some(vec![]),
            read_permission: Some(PlatformReadPermission::Deny),
            write_permission: Some(PlatformWritePermission::Ask),
        }));
    assert_eq!(
        serde_json::to_value(empty).unwrap(),
        json!({"builtin_skills":[],"platform_tools":{"groups":[],"read_permission":"deny","write_permission":"ask"}})
    );
    let selected = Overrides::new()
        .builtin_skills(Some(HashSet::from([BuiltinSkill::PlatformOperations])))
        .platform_tools(Some(PlatformSelection {
            groups: Some(vec![PlatformGroup::Execution]),
            ..Default::default()
        }));
    assert_eq!(
        serde_json::to_value(selected).unwrap(),
        json!({"builtin_skills":["platform-operations"],"platform_tools":{"groups":["execution"]}})
    );
    for wire in [
        json!({}),
        json!({"labels":null,"memory":null}),
        json!({"labels":{},"memory":{"enabled":false,"spaces":[]}}),
    ] {
        roundtrip::<m::SessionUpdate>(wire);
    }
    roundtrip::<m::SessionMemorySettings>(
        json!({"enabled":true,"spaces":[{"name":"shared","space_id":"space","access":"read"}]}),
    );
    roundtrip::<m::EnvironmentPermissionSetInput>(json!({"operations":[]}));
}

#[test]
fn run_memory_readback_preserves_legacy_and_resolved_roots_without_union_loss() {
    let legacy = json!({"name":"legacy","memory_id":"memory","access":"read","recall":false});
    let resolved = json!({"name":"shared","memory_id":"memory","access":"write","recall":false,"roots":["global","sessions/s"],"space_id":"space"});
    let mut run = sample("RunView");
    run["memory_mounts"] = json!([legacy, resolved]);
    roundtrip::<m::RunView>(run.clone());
    let typed: m::RunView = serde_json::from_value(run.clone()).unwrap();
    assert!(typed.memory_mounts[0].roots.is_none());
    assert_eq!(
        typed.memory_mounts[1].roots.as_ref().unwrap(),
        &["global", "sessions/s"]
    );
    assert_eq!(typed.memory_mounts[1].space_id, Some(Some("space".into())));
    for space_id in [None, Some(Value::Null), Some(json!("space"))] {
        let mut mount = json!({"name":"shared","memory_id":"memory","access":"write","roots":[]});
        if let Some(space_id) = space_id {
            mount["space_id"] = space_id;
        }
        run["memory_mounts"] = json!([mount]);
        roundtrip::<m::RunView>(run.clone());
    }
    for invalid in [Value::Null, json!(false), json!([1]), json!({})] {
        run["memory_mounts"] =
            json!([{"name":"shared","memory_id":"memory","access":"write","roots":invalid}]);
        assert!(serde_json::from_value::<m::RunView>(run.clone()).is_err());
    }
}

#[tokio::test]
async fn native_computer_mount_metrics_and_retained_image_stay_explicit_and_lossless() {
    let image = vec![0, 255, 137, 80, 78, 71, 13, 10, 0, 128];
    let image_reply = image.clone();
    let mut fixture = server(move |request| {
        if request.target.contains("/computer/check") {
            assert_eq!(request.method, "POST");
            assert!(request.body.is_empty());
            Reply::json(200,json!({"status":"not_ready","operations":[],"display_count":0,"observe_ready":false,"input_ready":false}))
        } else if request.target.contains("/images/") {
            let mut reply = Reply::bytes(200,"image/png",image_reply.clone());
            reply.headers.push_str("Cache-Control: no-store\r\n");
            reply
        } else if request.target.contains("/metrics") {
            Reply::json(200,json!({"time_to_first_chunk_seconds":null,"request_duration_seconds":0.0,"output_tokens_per_second":null}))
        } else {
            assert_eq!(request.method, "PATCH");
            assert_eq!(request.json(),json!({"permission_ceiling":{"operations":[]}}));
            assert!(request.headers.contains("if-match: \"thread:2\""));
            let mut mount = sample("MountView");
            mount["permission_ceiling"] = json!({"operations":[]});
            Reply::json(200,mount)
        }
    }).await;
    let sdk = client(&fixture);
    let checked = sdk
        .api()
        .environments()
        .at("env /")
        .computer()
        .check(Default::default())
        .await
        .unwrap();
    assert_eq!(checked.data.status, m::computer_check::Status::NotReady);
    assert!(
        fixture
            .requests
            .recv()
            .await
            .unwrap()
            .target
            .ends_with("/environments/env%20%2F/computer/check")
    );
    assert!(fixture.requests.try_recv().is_err()); // A probe does not acquire a desktop/image.
    let mut response = sdk
        .api()
        .runs()
        .at("r")
        .images()
        .at("image /")
        .get(Default::default())
        .await
        .unwrap();
    assert_eq!(response.status, reqwest::StatusCode::OK);
    assert_eq!(response.headers["content-type"], "image/png");
    assert_eq!(response.headers["cache-control"], "no-store");
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.unwrap() {
        bytes.extend_from_slice(&chunk);
    }
    assert_eq!(bytes, image);
    assert!(
        fixture
            .requests
            .recv()
            .await
            .unwrap()
            .target
            .ends_with("/runs/r/images/image%20%2F")
    );
    let metrics = sdk
        .api()
        .usage()
        .records()
        .at("record")
        .metrics()
        .get(Default::default())
        .await
        .unwrap();
    assert_eq!(metrics.data.time_to_first_chunk_seconds, None);
    assert_eq!(metrics.data.request_duration_seconds, Some(0.0));
    fixture.requests.recv().await.unwrap();
    let changed = sdk
        .api()
        .threads()
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
        .await
        .unwrap();
    assert!(
        changed
            .data
            .permission_ceiling
            .unwrap()
            .operations
            .unwrap()
            .is_empty()
    );
    let request = fixture.requests.recv().await.unwrap();
    assert!(request.headers.contains("authorization: bearer test-token"));
    assert!(fixture.requests.try_recv().is_err());
}
