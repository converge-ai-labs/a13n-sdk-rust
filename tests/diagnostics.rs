mod common;
use a13n::{
    Client, Error, ProtocolKind, TransportKind, TransportStage,
    resources::*,
    streaming::{StreamOptions, ThreadStream},
};
use common::{Reply, sample, server};
use std::{error::Error as _, time::Duration};

fn redacted(error: &Error) {
    for text in [
        error.to_string(),
        format!("{error:?}"),
        format!("{error:#?}"),
    ] {
        assert!(!text.contains("private"), "untrusted diagnostic data");
        assert!(!text.contains("127.0.0.1"), "transport URL retained");
    }
    let source = error.source().expect("safe typed diagnostic source");
    assert!(!source.to_string().contains("private"));
    assert!(source.source().is_none(), "raw transport cause retained");
}

#[tokio::test]
async fn transport_stages_categories_and_sources_are_safe() {
    let builder = Client::builder("http://localhost/private")
        .http_builder(reqwest::Client::builder().user_agent("private\nvalue"))
        .build()
        .err()
        .unwrap();
    assert!(
        matches!(&builder, Error::Transport(e) if e.stage == TransportStage::Build && e.kind == TransportKind::Builder)
    );
    redacted(&builder);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/private", listener.local_addr().unwrap());
    drop(listener);
    let client = Client::builder(base).build().unwrap();
    let error = client.resources().healthz().get().await.unwrap_err();
    assert!(
        matches!(&error, Error::Transport(e) if e.stage == TransportStage::Request && e.kind == TransportKind::Connect)
    );
    redacted(&error);

    for stage in [TransportStage::Request, TransportStage::Body] {
        let origin = server(move |_| {
            let mut reply = Reply::json(200, sample("Workspace"));
            if stage == TransportStage::Request {
                reply.delay = Duration::from_secs(20);
            } else {
                reply.chunks[0].0 = Duration::from_secs(20);
            }
            reply
        })
        .await;
        let client = Client::builder(&origin.url)
            .http_builder(
                reqwest::Client::builder()
                    .no_proxy()
                    .timeout(Duration::from_millis(50)),
            )
            .build()
            .unwrap();
        let error = client
            .resources()
            .workspaces()
            .at("private")
            .get()
            .await
            .unwrap_err();
        assert!(
            matches!(&error, Error::Transport(e) if e.stage == stage && e.kind == TransportKind::Timeout)
        );
        redacted(&error);
    }
}

#[tokio::test]
async fn response_diagnostics_preserve_metadata_without_printing_it() {
    for (body, kind) in [
        ("private".to_owned(), ProtocolKind::InvalidJson),
        ("x".repeat(65), ProtocolKind::ResponseTooLarge),
    ] {
        let origin = server(move |_| {
            let mut reply = Reply::bytes(200, "application/json", body.as_bytes().to_vec());
            reply.headers =
                "Content-Type: application/json\r\nX-Request-ID: private-request\r\n".into();
            reply
        })
        .await;
        let client = Client::builder(&origin.url)
            .response_limit(64)
            .build()
            .unwrap();
        let error = client
            .resources()
            .workspaces()
            .at("private")
            .get()
            .await
            .unwrap_err();
        let Error::Protocol(info) = &error else {
            panic!("protocol error")
        };
        assert_eq!(info.kind, kind);
        assert_eq!(info.status, Some(200));
        assert_eq!(info.request_id.as_deref(), Some("private-request"));
        redacted(&error);
    }
}

#[tokio::test]
async fn stream_diagnostics_do_not_change_recovery_or_cursor_semantics() {
    let origin = server(|_| Reply::bytes(200, "application/json", b"private".to_vec())).await;
    let client = common::client(&origin);
    let error = ThreadStream::open(
        client.resources().threads().at("t"),
        StreamOptions::default(),
    )
    .await
    .err()
    .unwrap();
    assert!(
        matches!(&error, Error::Protocol(e) if e.kind == ProtocolKind::UnexpectedContentType && e.status == Some(200))
    );
    redacted(&error);

    let origin = server(|_| Reply::sse("event: changed\ndata: private\n\n")).await;
    let client = common::client(&origin);
    let mut stream = ThreadStream::open(
        client.resources().threads().at("t"),
        StreamOptions::default(),
    )
    .await
    .unwrap();
    let error = stream.next().await.unwrap_err();
    assert!(matches!(&error, Error::Protocol(e) if e.kind == ProtocolKind::InvalidFrame));
    assert_eq!(stream.applied_cursor(), None);
    redacted(&error);
}

#[tokio::test]
async fn domain_enums_keep_wire_values_for_paths_and_queries() {
    let mut origin = server(|request| {
        Reply::json(
            200,
            if request.target.contains("provider-types") {
                sample("ProviderTypePage")
            } else {
                serde_json::json!({"items":[],"next_cursor":null})
            },
        )
    })
    .await;
    let client = common::client(&origin);
    // Query serialization uses the same serde representation as ordinary models.
    client
        .resources()
        .organizations()
        .at("o")
        .members()
        .list(OrganizationMembersListOptions {
            kind: Some(MemberKind::ServiceAccount),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        origin
            .requests
            .recv()
            .await
            .unwrap()
            .target
            .contains("kind=service_account")
    );
    client
        .resources()
        .skills()
        .list(SkillsListOptions {
            source: Some(SkillsSource::Github),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        origin
            .requests
            .recv()
            .await
            .unwrap()
            .target
            .contains("source=github")
    );
    client
        .resources()
        .provider_types()
        .at(ProviderKind::Memory)
        .list()
        .await
        .unwrap();
    assert!(
        origin
            .requests
            .recv()
            .await
            .unwrap()
            .target
            .ends_with("/provider-types/memory")
    );
    assert_eq!(ProviderKind::Memory.as_str(), "memory");
    assert_eq!(
        serde_json::to_string(&ProviderKind::Memory).unwrap(),
        "\"memory\""
    );
}
