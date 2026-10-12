mod common;
use a13n::{
    Answer, Error, ForkOptions, Input, InputPart, ObservationOptions, Overrides,
    PreviewInterruptedKind, ReceiptEvidence, ResumeOptions, RunConfiguration, RunOptions,
    RunStatus, SendOptions, StartOptions,
};
use common::{Reply, client, sample, server};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

fn receipt() -> Value {
    let mut v = sample("Submitted");
    v["thread"]["id"] = json!("t");
    v["thread"]["workspace_id"] = json!("w");
    v["entry"]["id"] = json!("e");
    v["entry"]["thread_id"] = json!("t");
    v["run"] = Value::Null;
    v
}
fn entry(status: &str, assigned: Option<&str>) -> Value {
    let mut v = sample("EntryView");
    v["id"] = json!("e");
    v["thread_id"] = json!("t");
    v["status"] = json!(status);
    v["assigned_run_id"] = json!(assigned);
    v
}
fn run(id: &str, status: &str, output: Value) -> Value {
    let mut v = sample("RunView");
    v["id"] = json!(id);
    v["thread_id"] = json!("t");
    v["status"] = json!(status);
    v["output"] = output;
    v
}
fn delta(sequence: i64, event: Value) -> String {
    format!(
        "event: delta\nid: {sequence}-0\ndata: {}\n\n",
        json!({"run_id":"r","attempt":1,"sequence":sequence,"event":event,"item":null})
    )
}
fn text_start(id: &str) -> Value {
    json!({"type":"TEXT_MESSAGE_START","messageId":id,"role":"assistant"})
}
fn text_content(id: &str, text: &str) -> Value {
    json!({"type":"TEXT_MESSAGE_CONTENT","messageId":id,"delta":text})
}

#[test]
fn public_input_and_every_options_builder_preserve_wire_presence() {
    let input = Input::parts([
        InputPart::text(""),
        InputPart::json(Value::Null),
        InputPart::url("https://example.test/i.png"),
        InputPart::asset("ast_1"),
    ]);
    assert_eq!(
        serde_json::to_value(input).unwrap(),
        json!({"content":[{"type":"text","text":""},{"type":"json","value":null},{"type":"url","url":"https://example.test/i.png"},{"type":"asset","asset_id":"ast_1"}]})
    );
    assert_eq!(serde_json::to_value(RunOptions::new()).unwrap(), json!({}));
    let overrides = Overrides::new()
        .client_tools(None)
        .connection_tools(None)
        .instructions(Some(String::new()))
        .media_understanding(None)
        .model(None)
        .model_characteristics(None)
        .model_settings(Some(HashMap::from([
            ("enabled".into(), json!(false)),
            ("zero".into(), json!(0)),
        ])))
        .output_spec(None)
        .plugins(None)
        .retries(None)
        .reviewer(None)
        .skills(None)
        .subagents(Some(json!({"nested":[null,{},[]]})))
        .toolsets(Some(HashMap::new()));
    let options = RunOptions::new()
        .configuration(Some(
            RunConfiguration::new()
                .allowed_hosts(Some(vec![]))
                .extensions(HashMap::from([(
                    "org.test".into(),
                    json!({"empty":"","flag":false}),
                )])),
        ))
        .overrides(Some(overrides))
        .labels(HashMap::new())
        .max_usage(None);
    assert_eq!(
        serde_json::to_value(options).unwrap(),
        json!({"configuration":{"allowed_hosts":[],"extensions":{"org.test":{"empty":"","flag":false}}},"labels":{},"max_usage":null,"overrides":{"client_tools":null,"connection_tools":null,"instructions":"","media_understanding":null,"model":null,"model_characteristics":null,"model_settings":{"enabled":false,"zero":0},"output_spec":null,"plugins":null,"retries":null,"reviewer":null,"skills":null,"subagents":{"nested":[null,{},[]]},"toolsets":{}}})
    );
    assert_eq!(
        serde_json::to_value(RunOptions::new().configuration(None).overrides(None)).unwrap(),
        json!({"configuration":null,"overrides":null})
    );
}

#[test]
fn uncommon_supported_options_are_constructible_without_generated_imports() {
    use a13n::{
        AgentModelCharacteristics, AgentReviewer, ClientToolDefinition, ClientToolPermission,
        ConnectionSelection, MediaUnderstandingSelection, MemoryAccess, MemoryMount, MountCreate,
        OutputSpec, OutputVariant, PluginSelection, RetryOverride, ReviewerOnError,
        ReviewerOnFlagged, RuleOnFlagged, SkillSelection, ToolPermissionSetting, ToolReviewRule,
        ToolRiskLevel, ToolsetSelection, UsageLimit,
    };
    let mut tool = ClientToolDefinition::new("description".into(), "lookup".into(), HashMap::new());
    tool.permission = Some(ClientToolPermission::Deny);
    let connection = ConnectionSelection {
        permission: Some(ToolPermissionSetting::Review),
        permissions: Some(HashMap::from([(
            "write".into(),
            ToolPermissionSetting::Ask,
        )])),
        ..ConnectionSelection::new("connection".into())
    };
    let reviewer = AgentReviewer {
        on_error: Some(ReviewerOnError::ApprovalRequired),
        on_flagged: Some(ReviewerOnFlagged::Deny),
        risk_threshold: Some(ToolRiskLevel::High),
        rules: Some(HashMap::from([(
            "write".into(),
            ToolReviewRule {
                on_flagged: Some(Some(RuleOnFlagged::ApprovalRequired)),
                risk_threshold: Some(Some(ToolRiskLevel::Medium)),
            },
        )])),
        ..AgentReviewer::new("reviewer-model".into())
    };
    let output = OutputSpec {
        variants: Some(Some(vec![
            OutputVariant::new("structured".into(), HashMap::new()),
            OutputVariant::new("alternate".into(), HashMap::new()),
        ])),
        ..Default::default()
    };
    let options = RunOptions::new()
        .overrides(Some(
            Overrides::new()
                .client_tools(Some(vec![tool]))
                .connection_tools(Some(vec![connection]))
                .instructions(Some("instructions".into()))
                .media_understanding(Some(MediaUnderstandingSelection::default()))
                .model(Some("model".into()))
                .model_characteristics(Some(AgentModelCharacteristics::default()))
                .model_settings(Some(HashMap::new()))
                .output_spec(Some(output))
                .plugins(Some(vec![PluginSelection::new(
                    "instance".into(),
                    "plugin".into(),
                )]))
                .retries(Some(RetryOverride::default()))
                .reviewer(Some(reviewer))
                .skills(Some(vec![SkillSelection::new("skill".into())]))
                .subagents(Some(json!({"child":{"agent_id":"agent"}})))
                .toolsets(Some(HashMap::from([(
                    "tools".into(),
                    ToolsetSelection::default(),
                )]))),
        ))
        .max_usage(Some(UsageLimit::new(10)));
    let wire = serde_json::to_value(&options).unwrap();
    assert_eq!(wire["overrides"]["client_tools"][0]["permission"], "deny");
    assert_eq!(
        wire["overrides"]["reviewer"]["rules"]["write"]["on_flagged"],
        "approval_required"
    );
    assert_eq!(wire["max_usage"]["requests"], 10);
    // Mount and Memory selectors also need no generated enum imports.
    let _start = StartOptions {
        environments: Some(vec![MountCreate::new("environment".into(), "work".into())]),
        memories: Some(vec![MemoryMount::new(
            MemoryAccess::Read,
            "memory".into(),
            "notes".into(),
        )]),
        run_options: Some(options),
        ..Default::default()
    };
}

#[tokio::test]
async fn native_api_and_authored_workflow_share_client_auth_and_shutdown() {
    let mut fixture = server(|request| {
        if request.method == "POST" {
            Reply::json(201, receipt())
        } else if request.target.ends_with("/inbox/e") {
            Reply::json(200, entry("consumed", Some("r")))
        } else {
            assert!(request.target.ends_with("/runs/r"));
            Reply::json(200, run("r", "completed", json!("same client")))
        }
    })
    .await;
    let sdk = client(&fixture);
    let api_run = sdk.api().runs().at("r");
    let native = api_run.get(Default::default()).await.unwrap();
    let clone = sdk.clone();
    let agent = clone.agent("a");
    drop(clone);
    let mut authored = agent.start("same client").await.unwrap();
    let outcome = authored.result().await.unwrap();
    assert_eq!(native.data.id, outcome.run_id);
    assert_eq!(native.data.output.as_deref(), Some(&outcome.output));
    let mut auth = None;
    for _ in 0..4 {
        let request = fixture.requests.recv().await.unwrap();
        let header = request
            .headers
            .lines()
            .find(|line| line.starts_with("authorization: "))
            .unwrap();
        if let Some(previous) = &auth {
            assert_eq!(header, previous);
        } else {
            auth = Some(header.to_owned());
        }
    }
    assert!(fixture.requests.try_recv().is_err());
    sdk.clone().close();
    assert!(matches!(
        api_run.get(Default::default()).await,
        Err(Error::Closed)
    ));
    assert!(matches!(agent.start("closed").await, Err(Error::Closed)));
    assert!(fixture.requests.try_recv().is_err());
}

#[tokio::test]
async fn direct_output_preserves_all_json_values_and_never_reads_items() {
    for output in [
        json!(""),
        json!("full reply"),
        json!(false),
        json!(0),
        Value::Null,
        json!([]),
        json!({}),
        json!({"nested":[null,false,0,""]}),
    ] {
        let expected = output.clone();
        let mut fixture = server(move |request| {
            assert!(request.target.ends_with("/runs/r"));
            Reply::json(200, run("r", "completed", output.clone()))
        })
        .await;
        let sdk = client(&fixture);
        let outcome = sdk.run("r").result(Default::default()).await.unwrap();
        assert_eq!(outcome.status, RunStatus::Completed);
        assert_eq!(outcome.output, expected);
        assert_eq!(outcome.text.as_deref(), expected.as_str());
        assert_eq!(outcome.decode_output::<Value>().unwrap(), expected);
        assert_eq!(outcome.run_id, "r");
        assert_eq!(outcome.thread_id, "t");
        assert_eq!(outcome.http_response.status, 200);
        assert_eq!(outcome.http_response.request_id(), Some("req_test"));
        fixture.requests.recv().await.unwrap();
        assert!(fixture.requests.try_recv().is_err());
    }
}

#[tokio::test]
async fn identity_recovery_is_local_and_only_consumption_binds_authority() {
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let mut fixture = server(move |request| {
        if request.target.ends_with("/inbox/e") {
            return Reply::json(
                200,
                match calls.fetch_add(1, Ordering::SeqCst) {
                    0 => entry("assigned", Some("provisional")),
                    1 => entry("pending", None),
                    _ => entry("consumed", Some("r")),
                },
            );
        }
        assert!(request.target.ends_with("/runs/r"));
        Reply::json(200, run("r", "waiting", Value::Null))
    })
    .await;
    let sdk = client(&fixture);
    let mut observation = sdk
        .entry("t", "e")
        .observe(ObservationOptions {
            poll_interval: Duration::from_millis(1),
            ..Default::default()
        })
        .unwrap();
    assert!(fixture.requests.try_recv().is_err());
    assert!(observation.run.is_none());
    assert!(observation.identity.run_id.is_none());
    assert!(matches!(
        observation.evidence,
        ReceiptEvidence::RecoveredIdentity
    ));
    let outcome = observation.result().await.unwrap();
    assert_eq!(outcome.run_id, "r");
    assert_eq!(observation.run.as_ref().unwrap().id, "r");
    assert_eq!(observation.identity.run_id.as_deref(), Some("r"));
    assert_eq!(count.load(Ordering::SeqCst), 3);
    for status in ["failed", "withdrawn"] {
        let fixture = server(move |_| Reply::json(200, entry(status, None))).await;
        let sdk = client(&fixture);
        assert!(
            matches!(sdk.entry("t", "e").observe(Default::default()).unwrap().result().await, Err(Error::Submission(e)) if serde_json::to_value(e.entry.data.status).unwrap() == status)
        );
    }
}

#[tokio::test]
async fn one_deadline_covers_queue_run_and_repeated_result_without_reset() {
    let fixture = server(|request| {
        let mut reply = if request.target.ends_with("/inbox/e") {
            Reply::json(200, entry("consumed", Some("r")))
        } else {
            Reply::json(200, run("r", "completed", json!("done")))
        };
        reply.delay = Duration::from_millis(45);
        reply
    })
    .await;
    let sdk = client(&fixture);
    let mut interaction = sdk
        .entry("t", "e")
        .observe(ObservationOptions {
            timeout: Duration::from_millis(65),
            ..Default::default()
        })
        .unwrap();
    assert!(matches!(interaction.result().await, Err(Error::Timeout)));
    assert!(matches!(interaction.result().await, Err(Error::Timeout)));
    let fresh = sdk
        .entry("t", "e")
        .observe(ObservationOptions {
            timeout: Duration::from_secs(1),
            ..Default::default()
        })
        .unwrap()
        .result()
        .await
        .unwrap();
    assert_eq!(fresh.text.as_deref(), Some("done"));
}

#[tokio::test]
async fn resume_answers_are_atomic_null_is_explicit_and_successor_reuses_interaction() {
    let events = delta(1, text_start("m")) + &delta(2, text_content("m", "preview"));
    let mut fixture = server(move |request| {
        assert!(!request.target.contains("/inbox/"));
        if request.method == "POST" {
            return Reply::json(201, run("r", "running", Value::Null));
        }
        if request.target.ends_with("/stream") {
            return Reply::sse(&events);
        }
        Reply::json(200, run("r", "completed", json!({"answer":42})))
    })
    .await;
    let sdk = client(&fixture);
    let answers = HashMap::from([
        ("approved".into(), Answer::approve()),
        ("denied".into(), Answer::deny()),
        ("denied_null".into(), Answer::deny_with(None)),
        ("denied_empty".into(), Answer::deny_with(Some("".into()))),
        ("returned".into(), Answer::returned(Value::Null)),
        ("failed".into(), Answer::failed("failed externally")),
    ]);
    let mut interaction = sdk
        .run("waiting")
        .resume_with(
            answers,
            ResumeOptions {
                idempotency_key: Some("resume-key".into()),
                input: Some(Some(Input::parts([
                    InputPart::text("extra"),
                    InputPart::json(json!(false)),
                ]))),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(interaction.run.as_ref().unwrap().id, "r");
    assert!(interaction.identity.entry_id.is_none());
    assert_eq!(interaction.evidence.resume().unwrap().status, 201);
    assert_eq!(
        interaction.next_text().await.unwrap().as_deref(),
        Some("preview")
    );
    let result = interaction.result().await.unwrap();
    assert_eq!(result.output, json!({"answer":42}));
    assert!(result.text.is_none());
    let sent = fixture.requests.recv().await.unwrap();
    assert_eq!(
        sent.json()["calls"]["returned"],
        json!({"status":"returned","value":null})
    );
    assert_eq!(sent.json()["approvals"]["denied"], json!({"action":"deny"}));
    assert_eq!(
        sent.json()["approvals"]["denied_null"],
        json!({"action":"deny","reason":null})
    );
    assert_eq!(
        sent.json()["approvals"]["denied_empty"],
        json!({"action":"deny","reason":""})
    );
    assert_eq!(
        sent.json()["input"]["content"][1],
        json!({"type":"json","value":false})
    );
}

#[tokio::test]
async fn optional_keys_are_chosen_once_exposed_and_unknown_outcomes_retain_cause() {
    let mut fixture = server(|_| Reply::json(201, receipt())).await;
    let sdk = client(&fixture);
    let first = sdk.agent("a").start("hi").await.unwrap();
    let second = sdk.agent("a").send("t", "again").await.unwrap();
    let a = first.identity.idempotency_key.unwrap();
    let b = second.identity.idempotency_key.unwrap();
    assert_ne!(a, b);
    assert_eq!(a.len(), 32);
    for key in [&a, &b] {
        assert!(
            fixture
                .requests
                .recv()
                .await
                .unwrap()
                .headers
                .contains(&format!("idempotency-key: {key}"))
        );
    }
    for malformed in [false, true] {
        let mut fixture = server(move |_| {
            if malformed {
                Reply::bytes(201, "application/json", b"{invalid".to_vec())
            } else {
                let mut reply = Reply::json(200, receipt());
                reply.disconnect = true;
                reply
            }
        })
        .await;
        let sdk = client(&fixture);
        let error = sdk
            .agent("a")
            .send_with(
                "t",
                "hi",
                SendOptions {
                    idempotency_key: Some("persisted-key".into()),
                    ..Default::default()
                },
            )
            .await
            .err()
            .unwrap();
        let Error::UnknownOutcome(error) = error else {
            panic!("Expected unknown outcome");
        };
        assert_eq!(error.idempotency_key, "persisted-key");
        assert_eq!(error.operation, "send");
        assert_eq!(error.thread_id.as_deref(), Some("t"));
        if malformed {
            assert!(
                matches!(error.cause, Error::Protocol(ref e) if e.status == Some(201) && e.request_id.as_deref() == Some("req_test"))
            );
        } else {
            assert!(matches!(error.cause, Error::Transport(_)));
        }
        fixture.requests.recv().await.unwrap();
        assert!(fixture.requests.try_recv().is_err());
    }
}

#[tokio::test]
async fn predispatch_errors_and_known_rejections_are_not_unknown() {
    let mut fixture = server(|_| {
        Reply::json(
            409,
            json!({"error":{"code":"conflict","message":"rejected","details":{},"request_id":null}}),
        )
    })
    .await;
    let sdk = client(&fixture);
    assert!(matches!(
        sdk.agent("a")
            .start_with(
                "hi",
                StartOptions {
                    idempotency_key: Some("".into()),
                    ..Default::default()
                }
            )
            .await,
        Err(Error::InvalidInput)
    ));
    assert!(
        matches!(sdk.agent("a").start_with("hi", StartOptions { idempotency_key: Some("bad\nheader".into()), ..Default::default() }).await, Err(Error::Transport(e)) if e.stage == a13n::TransportStage::Build)
    );
    assert!(fixture.requests.try_recv().is_err());
    assert!(matches!(sdk.agent("a").start("hi").await, Err(Error::Api(e)) if e.status == 409));
    fixture.requests.recv().await.unwrap();
    assert!(fixture.requests.try_recv().is_err());
    sdk.close();
    assert!(matches!(
        sdk.agent("a").start("hi").await,
        Err(Error::Closed)
    ));
}

async fn assert_request_id_acknowledgement(request_id: Option<Value>, known_rejection: bool) {
    let mut error = json!({"code":"conflict","message":"rejected","details":{}});
    if let Some(value) = request_id {
        error["request_id"] = value;
    }
    let mut fixture = server(move |_| Reply::json(409, json!({"error":error}))).await;
    let sdk = client(&fixture);
    let result = sdk.agent("a").start("hi").await;
    let request = fixture.requests.recv().await.unwrap();
    assert_eq!(request.method, "POST");
    let sent_key = request
        .headers
        .lines()
        .find_map(|line| line.strip_prefix("idempotency-key: "))
        .unwrap();
    assert_eq!(sent_key.len(), 32);
    if known_rejection {
        assert!(
            matches!(result, Err(Error::Api(ref e)) if e.status == 409 && e.code == "conflict")
        );
    } else {
        let Err(Error::UnknownOutcome(unknown)) = result else {
            panic!("Missing or invalid required request_id cannot acknowledge rejection");
        };
        assert_eq!(unknown.idempotency_key, sent_key);
        assert_eq!(unknown.operation, "start");
        assert_eq!(unknown.http_status, Some(409));
        assert!(
            matches!(unknown.cause, Error::Api(ref e) if e.status == 409 && e.code == "conflict" && e.request_id.as_deref() == Some("req_test"))
        );
    }
    assert!(fixture.requests.try_recv().is_err()); // Exactly one POST; no mutation retry.
}

#[tokio::test]
async fn rejection_missing_required_request_id_is_unknown_outcome() {
    assert_request_id_acknowledgement(None, false).await;
}

#[tokio::test]
async fn rejection_numeric_request_id_is_unknown_outcome() {
    assert_request_id_acknowledgement(Some(json!(123)), false).await;
}

#[tokio::test]
async fn rejection_explicit_null_request_id_is_known_api_rejection() {
    assert_request_id_acknowledgement(Some(Value::Null), true).await;
}

#[tokio::test]
async fn absolute_deadline_distinguishes_mutation_timeout_and_observation_timeout() {
    let mut fixture = server(|_| {
        let mut reply = Reply::json(201, receipt());
        reply.delay = Duration::from_millis(100);
        reply
    })
    .await;
    let sdk = client(&fixture);
    let error = sdk
        .agent("a")
        .start_with(
            "hi",
            StartOptions {
                idempotency_key: Some("k".into()),
                observation: ObservationOptions {
                    deadline: Some(tokio::time::Instant::now() + Duration::from_millis(25)),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
        .err()
        .unwrap();
    assert!(
        matches!(error, Error::UnknownOutcome(e) if matches!(e.cause, Error::MutationTimeout) && e.idempotency_key == "k")
    );
    fixture.requests.recv().await.unwrap();
    assert!(matches!(
        sdk.agent("a")
            .start_with(
                "hi",
                StartOptions {
                    observation: ObservationOptions {
                        deadline: Some(tokio::time::Instant::now()),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            )
            .await,
        Err(Error::MutationTimeout)
    ));
    assert!(fixture.requests.try_recv().is_err());
}

#[tokio::test]
async fn owned_handles_survive_clone_drop_and_explicit_close_is_shared_and_task_safe() {
    let mut fixture = server(|request| {
        if request.method == "POST" {
            return Reply::json(201, receipt());
        }
        if request.target.ends_with("/inbox/e") {
            return Reply::json(200, entry("consumed", Some("r")));
        }
        Reply::json(200, run("r", "running", Value::Null))
    })
    .await;
    let sdk = client(&fixture);
    let closing = sdk.clone();
    let agent = sdk.agent("a");
    drop(sdk);
    let mut interaction = tokio::spawn(async move { agent.start("hi").await.unwrap() })
        .await
        .unwrap();
    fixture.requests.recv().await.unwrap();
    let pending = tokio::spawn(async move { interaction.result().await });
    fixture.requests.recv().await.unwrap();
    fixture.requests.recv().await.unwrap();
    closing.close();
    assert!(matches!(pending.await.unwrap(), Err(Error::Closed)));
    assert!(matches!(
        closing.run("r").result(Default::default()).await,
        Err(Error::Closed)
    ));
}

#[tokio::test]
async fn fork_exposes_public_input_options_and_normal_entry_incorporation() {
    let mut fixture = server(|request| {
        if request.method == "POST" {
            return Reply::json(201, receipt());
        }
        if request.target.ends_with("/inbox/e") {
            return Reply::json(200, entry("consumed", Some("r")));
        }
        Reply::json(200, run("r", "completed", json!("forked")))
    })
    .await;
    let sdk = client(&fixture);
    let mut interaction = sdk
        .run("source")
        .fork(
            "agent",
            Input::parts([InputPart::text("continue")]),
            ForkOptions {
                idempotency_key: Some("fork-key".into()),
                fresh_environments: Some(false),
                run_options: Some(RunOptions::new().configuration(None)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(interaction.run.is_none());
    assert_eq!(
        interaction.result().await.unwrap().text.as_deref(),
        Some("forked")
    );
    let sent = fixture.requests.recv().await.unwrap();
    assert!(sent.target.ends_with("/runs/source/fork"));
    assert_eq!(sent.json()["fresh_environments"], false);
    assert_eq!(sent.json()["options"], json!({"configuration":null}));
}

#[tokio::test]
async fn preview_filters_child_hidden_foreign_and_unrelated_custom_without_false_holes() {
    let events = [
        text_start("one"),
        json!({"type":"TEXT_MESSAGE_START","messageId":"one","role":"assistant","subagentRunId":"child"}),
        json!({"type":"TEXT_MESSAGE_CONTENT","messageId":"one","delta":"child","subagentRunId":"child"}),
        json!({"type":"CUSTOM","name":"a13n.stream.fragment","value":{"id":"opaque","index":0,"count":2,"data":"{\"type\":\"CUSTOM\""},"subagentRunId":"child"}),
        json!({"type":"CUSTOM","name":"a13n.stream.fragment","metadata":{"display":false},"value":{"id":"hidden","index":0,"count":2,"data":"opaque"}}),
        json!({"type":"CUSTOM","name":"unknown.future","value":{"sequence":9999,"attempt":9}}),
        json!({"type":"TEXT_MESSAGE_CONTENT","messageId":"one","delta":"hidden","metadata":{"display":false}}),
        text_start("two"),
        text_content("two", "B"),
        text_content("one", "A"),
    ];
    let mut wire = String::new();
    for (i, event) in events.into_iter().enumerate() {
        wire.push_str(&delta(i as i64 + 1, event));
    }
    wire.insert_str(0, &format!("event: delta\nid: 0-0\ndata: {}\n\n",json!({"run_id":"foreign","attempt":90,"sequence":1000,"event":text_content("missing","foreign"),"item":null})));
    let fixture = server(move |request| {
        if request.target.ends_with("/inbox/e") {
            return Reply::json(200, entry("consumed", Some("r")));
        }
        if request.target.ends_with("/stream") {
            return Reply::sse(&wire);
        }
        Reply::json(200, run("r", "completed", json!("authoritative")))
    })
    .await;
    let sdk = client(&fixture);
    let mut interaction = sdk.entry("t", "e").observe(Default::default()).unwrap();
    assert_eq!(interaction.next_text().await.unwrap().as_deref(), Some("B"));
    assert_eq!(interaction.next_text().await.unwrap().as_deref(), Some("A"));
    assert!(interaction.next_text().await.unwrap().is_none());
    assert_eq!(
        interaction.result().await.unwrap().text.as_deref(),
        Some("authoritative")
    );
}

#[tokio::test]
async fn preview_interruptions_are_typed_sticky_and_keep_result_and_sequential_raw() {
    let cases = [
        (
            "event: gap\ndata: {\"run_id\":\"r\",\"position\":null}\n\n".to_owned(),
            PreviewInterruptedKind::Gap,
        ),
        (
            "event: reset\ndata: {\"run_id\":\"r\"}\n\n".to_owned(),
            PreviewInterruptedKind::Reset,
        ),
        (
            delta(
                1,
                json!({"type":"CUSTOM","name":"a13n.stream.fragment","value":{"id":"opaque","index":0,"count":2,"data":"{\"type\":\"CUSTOM\""}}),
            ),
            PreviewInterruptedKind::Fragment,
        ),
        (
            delta(1, text_content("missing", "unsafe")),
            PreviewInterruptedKind::MissingStart,
        ),
        (
            delta(1, text_start("m")) + &delta(3, text_content("m", "hole")),
            PreviewInterruptedKind::Discontinuity,
        ),
        (
            (1..=9)
                .map(|i| delta(i, text_start(&format!("m{i}"))))
                .collect(),
            PreviewInterruptedKind::Capacity,
        ),
        (
            delta(1, text_start("m")) + &delta(2, text_start("m")),
            PreviewInterruptedKind::AmbiguousIdentity,
        ),
    ];
    for (wire, kind) in cases {
        let wire = wire
            + &delta(
                50,
                json!({"type":"CUSTOM","name":"raw-after-interruption","value":false}),
            );
        let fixture = server(move |request| {
            if request.target.ends_with("/inbox/e") {
                return Reply::json(200, entry("consumed", Some("r")));
            }
            if request.target.ends_with("/stream") {
                return Reply::sse(&wire);
            }
            Reply::json(200, run("r", "completed", json!("durable")))
        })
        .await;
        let sdk = client(&fixture);
        let mut interaction = sdk.entry("t", "e").observe(Default::default()).unwrap();
        assert!(
            matches!(interaction.next_text().await, Err(Error::PreviewInterrupted(e)) if e.kind == kind)
        );
        assert!(
            matches!(interaction.next_text().await, Err(Error::PreviewInterrupted(e)) if e.kind == kind)
        );
        assert!(
            matches!(interaction.next().await.unwrap(), Some(a13n::streaming::ThreadFrame::Delta { data, .. }) if data.event["name"] == "raw-after-interruption")
        );
        assert_eq!(
            interaction.result().await.unwrap().text.as_deref(),
            Some("durable")
        );
    }
}

#[tokio::test]
async fn late_covered_checkpoint_does_not_regress_dense_preview_coordinate() {
    let wire = delta(1, text_start("m"))
        + &delta(2, text_content("m", "A"))
        + "event: boundary\nid: 20-0\ndata: {\"run_id\":\"r\",\"attempt\":1,\"sequence\":1}\n\n"
        + &delta(3, text_content("m", "B"));
    let fixture = server(move |request| {
        if request.target.ends_with("/inbox/e") {
            return Reply::json(200, entry("consumed", Some("r")));
        }
        if request.target.ends_with("/stream") {
            return Reply::sse(&wire);
        }
        Reply::json(200, run("r", "completed", json!("AB")))
    })
    .await;
    let sdk = client(&fixture);
    let mut interaction = sdk.entry("t", "e").observe(Default::default()).unwrap();
    assert_eq!(interaction.next_text().await.unwrap().as_deref(), Some("A"));
    assert_eq!(interaction.next_text().await.unwrap().as_deref(), Some("B"));
    assert!(interaction.next_text().await.unwrap().is_none());
    assert_eq!(
        interaction.result().await.unwrap().text.as_deref(),
        Some("AB")
    );
}

#[tokio::test]
async fn gateway_status_without_canonical_rejection_keeps_key_and_actual_status() {
    for body_times_out in [false, true] {
        let mut fixture = server(move |_| {
            let mut reply = Reply::bytes(504, "text/html", b"<h1>gateway timeout</h1>".to_vec());
            if body_times_out {
                reply
                    .chunks
                    .push((Duration::from_millis(100), b"tail".to_vec()));
            }
            reply
        })
        .await;
        let sdk = a13n::Client::builder(&fixture.url)
            .http_builder(
                reqwest::Client::builder()
                    .no_proxy()
                    .timeout(Duration::from_millis(40)),
            )
            .build()
            .unwrap();
        let error = sdk
            .agent("a")
            .start_with(
                "hi",
                StartOptions {
                    idempotency_key: Some("gateway-intent".into()),
                    ..Default::default()
                },
            )
            .await
            .err()
            .unwrap();
        let Error::UnknownOutcome(error) = error else {
            panic!("Gateway status is not an authoritative rejection");
        };
        assert_eq!(error.idempotency_key, "gateway-intent");
        assert_eq!(error.http_status, Some(504));
        if body_times_out {
            assert!(matches!(error.cause, Error::Transport(_)));
        } else {
            assert!(
                matches!(error.cause, Error::Api(e) if e.status == 504 && e.code == "http_error")
            );
        }
        fixture.requests.recv().await.unwrap();
        assert!(fixture.requests.try_recv().is_err());
    }
}

#[tokio::test]
async fn boundary_at_high_water_continues_but_ahead_boundary_cannot_heal_missing_delta() {
    for boundary in [2, 3] {
        let wire = delta(1, text_start("m"))
            + &delta(2, text_content("m", "A"))
            + &format!(
                "event: boundary\nid: 20-0\ndata: {{\"run_id\":\"r\",\"attempt\":1,\"sequence\":{boundary}}}\n\n"
            )
            + &delta(3, text_content("m", "B"));
        let fixture = server(move |request| {
            if request.target.ends_with("/inbox/e") {
                return Reply::json(200, entry("consumed", Some("r")));
            }
            if request.target.ends_with("/stream") {
                return Reply::sse(&wire);
            }
            Reply::json(200, run("r", "completed", json!("AB")))
        })
        .await;
        let sdk = client(&fixture);
        let mut interaction = sdk.entry("t", "e").observe(Default::default()).unwrap();
        assert_eq!(interaction.next_text().await.unwrap().as_deref(), Some("A"));
        if boundary == 2 {
            assert_eq!(interaction.next_text().await.unwrap().as_deref(), Some("B"));
        } else {
            assert!(
                matches!(interaction.next_text().await, Err(Error::PreviewInterrupted(e)) if e.kind == PreviewInterruptedKind::Discontinuity)
            );
        }
        assert_eq!(
            interaction.result().await.unwrap().text.as_deref(),
            Some("AB")
        );
    }
}
