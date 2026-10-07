mod common;
use a13n::{
    Error,
    generated::models,
    resources::RunItemsGetOptions,
    streaming::{StreamOptions, ThreadFrame, ThreadStream},
};
use common::*;
use serde_json::{Value, json};

fn continuation() -> Value {
    json!({"run_id":"r","position":{"attempt":1,"sequence":9},"next_ordinal":10,
        "full_content":false,"response_groups":{"child":"g"},
        "arguments":{"at":"2026-10-06T00:00:00Z","event":{"custom":[false,null,{"empty":[]}]},"key":"call","sequence":8,"size":0,"stream":null},
        "fragments":{"gap":false,"max_bytes":0,"max_pending":0,"pending":{"custom":{"count":2,"parts":["{","}"],"size":2}}},
        "observer":{"run_id":null,"thread_id":"t","state":{"request_index":0,"threads":{"child":"ct"},"parts":{"p":{"kind":"tool_call","part_id":"p","emitted_content":false,"tool_name":null}},"children":{"child":{"children":{"grandchild":{"parts":{},"threads":{},"request_index":1}}}}}}})
}
fn window(baseline: bool, complete: bool, ordinals: &[i32]) -> Value {
    let mut value = sample("RunItems");
    value["run"]["id"] = json!("r");
    value["run"]["status"] = json!(if complete { "completed" } else { "running" });
    value["run"]["display_position"] = json!("1-9");
    value["baseline"] = json!(baseline);
    value["complete"] = json!(complete);
    value["position"] = if baseline { json!("1-9") } else { Value::Null };
    value["resume_after"] = if baseline {
        json!("100-9")
    } else {
        Value::Null
    };
    value["continuation"] = if baseline {
        continuation()
    } else {
        Value::Null
    };
    value["items"] = Value::Array(
        ordinals
            .iter()
            .map(|ordinal| {
                let mut item = sample("Item");
                item["id"] = json!(format!("i{ordinal}"));
                item["ordinal"] = json!(ordinal);
                item["content"] = json!({"subagentRunId":"child","future":[false,null]});
                item
            })
            .collect(),
    );
    value
}
#[test]
fn recursive_display_state_and_required_window_fields_roundtrip() {
    for value in [
        window(true, false, &[6, 7, 8, 9]),
        window(true, true, &[8, 9]),
        window(false, true, &[4, 5]),
    ] {
        let parsed: models::RunItems = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), value);
    }
    for key in ["baseline", "complete", "position", "items"] {
        let mut value = window(true, true, &[8, 9]);
        value.as_object_mut().unwrap().remove(key);
        assert!(
            serde_json::from_value::<models::RunItems>(value).is_err(),
            "{key}"
        );
    }
    let mut item = sample("Item");
    item.as_object_mut().unwrap().remove("ordinal");
    assert!(serde_json::from_value::<models::Item>(item).is_err());
    for state in [None, Some(Value::Null), Some(continuation())] {
        let mut value = window(true, false, &[]);
        value.as_object_mut().unwrap().remove("continuation");
        if let Some(state) = state {
            value["continuation"] = state;
        }
        let parsed: models::RunItems = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), value);
    }
}
#[tokio::test]
async fn ordinal_windows_preserve_whole_tail_and_do_not_autoload_history_or_claim_live_coverage() {
    let mut fixture = server(|request| {
        let body = if request.target.ends_with("?limit=2") {
            window(true, false, &[6, 7, 8, 9])
        } else if request.target.ends_with("?before=6&limit=2") {
            window(false, true, &[4, 5])
        } else if request.target.ends_with("?after=7&limit=2") {
            window(false, true, &[8, 9])
        } else if request.target.ends_with("/stream") {
            return Reply::sse("event: gap\ndata: {\"run_id\":\"r\",\"position\":\"1-10\"}\n\n");
        } else {
            panic!("unexpected read {}", request.target)
        };
        Reply::json(200, body)
    })
    .await;
    let sdk = client(&fixture);
    let run = sdk.run("r");
    let tail = run
        .items()
        .get(RunItemsGetOptions {
            limit: Some(2),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(tail.data.baseline && !tail.data.complete);
    assert_eq!(tail.data.items.len(), 4); // The SDK must not truncate the entire mutable tail to limit.
    assert_eq!(tail.data.items[0].ordinal, 6);
    for options in [
        RunItemsGetOptions {
            before: Some(6),
            limit: Some(2),
            ..Default::default()
        },
        RunItemsGetOptions {
            after: Some(7),
            limit: Some(2),
            ..Default::default()
        },
    ] {
        let page = run.items().get(options).await.unwrap().data;
        assert!(page.complete && !page.baseline); // Sealed is not all-history loaded.
        assert!(page.items[0].ordinal > 1);
        assert!(
            page.position.is_none()
                && page.continuation == Some(None)
                && page.resume_after == Some(None)
        );
    }
    let mut reader =
        ThreadStream::open(sdk.resources().threads().at("t"), StreamOptions::default())
            .await
            .unwrap();
    assert!(matches!(
        reader.next().await.unwrap(),
        Some(ThreadFrame::Gap(_))
    ));
    assert_eq!(reader.applied_position(), None);
    reader.close();
    for expected in [
        "/prefix/api/v1/runs/r/items?limit=2",
        "/prefix/api/v1/runs/r/items?before=6&limit=2",
        "/prefix/api/v1/runs/r/items?after=7&limit=2",
        "/prefix/api/v1/threads/t/stream",
    ] {
        let request = fixture.requests.recv().await.unwrap();
        assert_eq!(request.target, expected);
        assert_eq!(request.method, "GET");
    }
    assert!(fixture.requests.try_recv().is_err());
}
#[tokio::test]
async fn ordinal_query_bounds_and_exclusion_are_service_errors_not_sdk_paging_policy() {
    let mut fixture = server(|_|Reply::json(400,json!({"error":{"code":"invalid_argument","message":"ordinal window","details":{"reason":"invalid_window"}}}))).await;
    let sdk = client(&fixture);
    for (options, query) in [
        (
            RunItemsGetOptions {
                before: Some(0),
                ..Default::default()
            },
            "before=0",
        ),
        (
            RunItemsGetOptions {
                after: Some(-1),
                ..Default::default()
            },
            "after=-1",
        ),
        (
            RunItemsGetOptions {
                limit: Some(501),
                ..Default::default()
            },
            "limit=501",
        ),
        (
            RunItemsGetOptions {
                before: Some(1),
                after: Some(0),
                limit: Some(0),
                x_workspace_id: Some("w".into()),
            },
            "before=1&after=0&limit=0",
        ),
    ] {
        assert!(
            matches!(sdk.run("r /+").items().get(options).await,Err(Error::Api(ref error)) if error.status == 400 && error.details["reason"] == "invalid_window")
        );
        let request = fixture.requests.recv().await.unwrap();
        assert_eq!(
            request.target,
            format!("/prefix/api/v1/runs/r%20%2F+/items?{query}")
        );
        assert!(request.headers.contains("authorization:"));
        if query.contains("after=0") {
            assert!(request.headers.contains("x-workspace-id: w"));
        }
    }
    assert!(fixture.requests.try_recv().is_err());
}

#[tokio::test]
async fn sealed_historical_readback_cannot_heal_or_seal_an_existing_gapped_reader() {
    let mut fixture = server(|request| {
        if request.target.ends_with("/items?after=0&limit=2") {
            Reply::json(200, window(false, true, &[1,2]))
        } else {
            assert!(request.target.ends_with("/stream?run=r&position=1-9"));
            Reply::sse("event: gap\ndata: {\"run_id\":\"r\",\"position\":\"1-10\"}\n\nevent: delta\nid: 100-10\ndata: {\"run_id\":\"r\",\"attempt\":1,\"sequence\":10,\"event\":{},\"item\":null}\n\nevent: boundary\nid: 100-10\ndata: {\"run_id\":\"r\",\"attempt\":1,\"sequence\":10}\n\n")
        }
    }).await;
    let sdk = client(&fixture);
    let mut reader = ThreadStream::open(
        sdk.resources().threads().at("t"),
        StreamOptions {
            run: Some("r".into()),
            position: Some("1-9".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        reader.next().await.unwrap(),
        Some(ThreadFrame::Gap(_))
    ));
    let history = sdk
        .run("r")
        .items()
        .get(RunItemsGetOptions {
            after: Some(0),
            limit: Some(2),
            ..Default::default()
        })
        .await
        .unwrap()
        .data;
    assert!(history.complete && !history.baseline && history.position.is_none());
    assert!(matches!(
        reader.next().await.unwrap(),
        Some(ThreadFrame::Delta { .. })
    ));
    assert!(matches!(
        reader.next().await.unwrap(),
        Some(ThreadFrame::Boundary { .. })
    ));
    assert_eq!(reader.applied_position(), Some("1-9"));
    assert_eq!(
        fixture.requests.recv().await.unwrap().target,
        "/prefix/api/v1/threads/t/stream?run=r&position=1-9"
    );
    assert_eq!(
        fixture.requests.recv().await.unwrap().target,
        "/prefix/api/v1/runs/r/items?after=0&limit=2"
    );
    assert!(fixture.requests.try_recv().is_err());
    reader.close();
}
