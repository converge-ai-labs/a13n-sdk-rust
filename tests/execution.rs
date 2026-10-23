mod common;
use a13n::{
    ExecutionOverride, RunOptions, SendOptions, StartOptions, generated::models as m,
    resources::ThreadUpdateOptions,
};
use common::{Reply, client, sample, server};
use serde_json::{Value, json};

#[tokio::test]
async fn execution_presence_survives_start_send_and_thread_patch() {
    for expected in [
        json!({}),
        json!({"execution":null}),
        json!({"execution":{}}),
        json!({"execution":{"environment":null}}),
        json!({"execution":{"environment":{}}}),
        json!({"execution":{"tools":{"mode":"auto","foreground_wait_seconds":0.0},"steering":"boundary","environment":{"APP_ENV":"test","EMPTY":""}}}),
    ] {
        let execution: Option<Option<ExecutionOverride>> = expected.get("execution").map(|v| {
            if v.is_null() {
                None
            } else {
                Some(serde_json::from_value(v.clone()).unwrap())
            }
        });
        let options = match execution.clone() {
            None => RunOptions::new(),
            Some(value) => RunOptions::new().execution(value),
        };
        let mut srv = server(|request| {
            if request.method == "PATCH" {
                return Reply::json(200, sample("ThreadView"));
            }
            let mut receipt = sample("Submitted");
            receipt["thread"]["id"] = json!("t");
            receipt["entry"]["thread_id"] = json!("t");
            receipt["run"] = Value::Null;
            Reply::json(201, receipt)
        })
        .await;
        let c = client(&srv);
        let mut first = c
            .agent("agent")
            .start_with(
                "x",
                StartOptions {
                    execution,
                    run_options: Some(options.clone()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let mut next = c
            .agent("agent")
            .send_with(
                "t",
                "x",
                SendOptions {
                    run_options: Some(options),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let update: m::ThreadUpdate = serde_json::from_value(expected.clone()).unwrap();
        c.api()
            .threads()
            .at("t")
            .update(
                &update,
                ThreadUpdateOptions {
                    if_match: "\"v1\"".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let first_body = srv.requests.recv().await.unwrap().json();
        let next_body = srv.requests.recv().await.unwrap().json();
        let patch = srv.requests.recv().await.unwrap();
        assert_eq!(first_body.get("execution"), expected.get("execution"));
        assert_eq!(first_body["options"], expected);
        assert_eq!(next_body["options"], expected);
        assert!(next_body.get("execution").is_none());
        assert_eq!(patch.json(), expected);
        assert!(patch.headers.contains("if-match: \"v1\""));
        first.close();
        next.close();
    }
}

#[test]
fn public_execution_types_cover_agent_configuration() {
    use a13n::{Steering, ToolExecutionMode, ToolExecutionOverride};
    let execution = ExecutionOverride {
        environment: Some(Some([("EMPTY".into(), "".into())].into())),
        tools: Some(Some(Box::new(ToolExecutionOverride {
            mode: Some(Some(ToolExecutionMode::Inline)),
            foreground_wait_seconds: Some(Some(0.0)),
        }))),
        steering: Some(Some(Steering::Immediate)),
    };
    let config = m::AgentConfigInput {
        execution: Some(Some(Box::new(execution))),
        ..Default::default()
    };
    let value = serde_json::to_value(config).unwrap();
    assert_eq!(value["execution"]["environment"]["EMPTY"], json!(""));
    assert_eq!(
        value["execution"]["tools"]["foreground_wait_seconds"],
        json!(0.0)
    );
}

#[tokio::test]
async fn observers_request_detailed_and_reject_fragments() {
    let fixture = server(|request| {
        assert!(request.target.contains("view=detailed"));
        Reply::json(200, sample("TextRead"))
    })
    .await;
    let sdk = client(&fixture);
    let mut run = sdk.run("run").observe(Default::default()).unwrap();
    assert!(matches!(run.result().await, Err(a13n::Error::Protocol(_))));
    let mut entry = sdk
        .entry("thread", "entry")
        .observe(Default::default())
        .unwrap();
    assert!(matches!(
        entry.result().await,
        Err(a13n::Error::Protocol(_))
    ));
}
