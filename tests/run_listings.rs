mod common;
use a13n::resources::ThreadRunsListOptions;
use common::*;
use serde_json::json;

#[tokio::test]
async fn anchored_run_pages_are_lazy_and_drop_the_anchor_when_following_a_cursor() {
    let mut fixture = server(|request| {
        let mut item = sample("RunListItem");
        item["input_preview"] = json!({"text":"😀".repeat(200),"truncated":true});
        Reply::json(200, json!({"items":[item],"total":10000,"first_number":501,
            "previous_cursor":"newer","next_cursor":if request.target.contains("cursor=older") {None} else {Some("older")}}))
    }).await;
    let sdk = client(&fixture);
    let mut pages = sdk
        .api()
        .threads()
        .at("thr_1")
        .runs()
        .pages(ThreadRunsListOptions {
            anchor: Some("run_1".into()),
            limit: Some(2),
            ..Default::default()
        });
    assert!(fixture.requests.try_recv().is_err());
    let page = pages.next().await.unwrap().unwrap().data;
    assert_eq!(page.total, 10000);
    assert_eq!(page.first_number, 501);
    assert_eq!(page.previous_cursor.as_deref(), Some("newer"));
    assert_eq!(page.items[0].input_preview.text, "😀".repeat(200));
    assert!(page.items[0].input_preview.truncated);
    let item = serde_json::to_value(&page.items[0]).unwrap();
    assert!(item.get("output").is_none() && item.get("options").is_none());
    assert!(pages.next().await.unwrap().is_some());
    assert!(pages.next().await.unwrap().is_none());
    for (anchor, cursor) in [(Some("run_1"), None), (None, Some("older"))] {
        let request = fixture.requests.recv().await.unwrap();
        let url = reqwest::Url::parse(&format!("http://localhost{}", request.target)).unwrap();
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query.get("anchor").map(String::as_str), anchor);
        assert_eq!(query.get("cursor").map(String::as_str), cursor);
        assert_eq!(query.get("limit").map(String::as_str), Some("2"));
        assert!(!query.contains_key("view"));
    }
    assert!(fixture.requests.try_recv().is_err());
}
