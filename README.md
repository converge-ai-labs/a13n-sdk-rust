# a13n

Async Rust SDK for a13n Service. `Client::resources()` exposes every operation in the pinned OpenAPI contract: 230 operations across 154 paths, including workspace and organization administration, agents, threads, runs, assets, environments, memories and provider accounts. The same contract generates advanced HTTP bindings and complete wire models.

The ordinary resource graph is the primary API. Thin helpers add acceptance references, exact Run/Entry waiting, lazy pagination and typed Thread SSE without implementing Service business logic.

## Installation

The crate is named `a13n`. Use a published version when available; the source version `0.0.0` is a development placeholder, not a release promise. This repository also contains the separately versioned [CLI](a13n-service-cli/README.md).

## Submit and wait

Bind workspace IDs or keys explicitly. Binding is local and never discovers or expands credential authority. Resource references borrow their client and can be reused without keeping intermediate collections alive.

```rust,no_run
use a13n::{Client, Secret, generated::models, resources::ThreadsCreateOptions, text_payload};
use std::time::Duration;

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let client = Client::new("https://service.example", Secret::new(std::env::var("A13N_TOKEN")?))?;
let workspace = client.resources().workspaces().at("my-workspace");
let body = models::NewThread::new("agt_example".into(), text_payload("Summarize this project"));
let submitted = workspace.threads().create(&body, ThreadsCreateOptions {
    idempotency_key: "request-unique-key".into(),
}).await?;
println!("HTTP {}", submitted.receipt.status);

// Acceptance is not completion. Use one deadline for queue and Run observation.
let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
let poll = Duration::from_millis(250);
let run = match submitted.run {
    Some(run) => run,
    None => {
        let entry = tokio::time::timeout_at(deadline, submitted.entry.wait(poll)).await??;
        if entry.data.status != models::EntryStatus::Consumed {
            return Err(std::io::Error::other(format!("Entry ended: {:?}", entry.data.status)).into());
        }
        let id = entry.data.assigned_run_id
            .ok_or_else(|| std::io::Error::other("Consumed Entry has no assigned Run"))?;
        client.resources().workspaces()
            .at(submitted.receipt.data.thread.workspace_id.clone()).runs().at(id)
    }
};
let result = tokio::time::timeout_at(deadline, run.wait(poll)).await?;
match result?.data.status {
    models::RunStatus::Completed => println!("Run completed; inspect its Items for output"),
    models::RunStatus::Waiting => println!("Run needs input; inspect its pending interactions"),
    models::RunStatus::Failed | models::RunStatus::Cancelled => println!("Run did not complete successfully"),
    status => println!("Observed status: {status:?}"),
}
client.close();
# Ok(())
# }
```

Use the full `NewThread` or `Message` model for parts, options, environments, memories and other input. `text_payload` is just a text convenience. `Submitted` retains `.receipt`, `.thread`, `.entry` and optional `.run`, bound to the receipt's canonical workspace identity. Inbox submission and fork use the same return shape and preserve fresh 201 versus replay 200. Resume returns a `Response<models::RunView>` for the successor: bind `workspace.runs().at(resumed.data.id)` to wait for it. Waiting on the original Run does not follow that successor.

`run.wait` observes only that Run until completed, waiting, failed or cancelled. It does not follow successors or declare business success. `entry.wait` observes consumed, failed or withdrawn, not merely assignment. Wrapping the entire future in a timeout bounds requests, response bodies and polling sleeps.

### Resume a waiting Run

Supply answers appropriate to the returned pending interactions; do not fabricate approvals or tool results. Resume and observation are separate operations with caller-owned request keys and deadlines.

```rust,no_run
use a13n::{Client, generated::models, resources::RunResumeOptions};
use std::time::Duration;

# async fn resume(client: &Client, workspace_id: &str, run_id: &str, answers: Vec<models::Answer>) -> Result<(), Box<dyn std::error::Error>> {
let workspace = client.resources().workspaces().at(workspace_id);
let source = workspace.runs().at(run_id);
let resumed = source.resume(&models::ResumeRequest { answers: Some(answers) }, RunResumeOptions {
    idempotency_key: "resume-unique-key".into(),
}).await?;
let successor = workspace.runs().at(resumed.data.id);
let observed = tokio::time::timeout(
    Duration::from_secs(60), successor.wait(Duration::from_millis(250)),
).await??;
println!("Successor status: {:?}", observed.data.status);
// Still inspect status: waiting/failed/cancelled is not business success.
# Ok(())
# }
```

## Read, update and paginate

```rust,no_run
use a13n::{Client, generated::models, resources::{AgentUpdateOptions, AgentsListOptions}};

# async fn example(client: &Client) -> Result<(), a13n::Error> {
let workspace = client.resources().workspaces().at("ws_example");
let agent = workspace.agents().at("agt_example");
let current = agent.get().await?;
let updated = agent.update(&models::AgentUpdate {
    name: Some(Some("Support".into())),
    description: Some(None), // explicit JSON null; None would omit the field
    ..Default::default()
}, AgentUpdateOptions {
    if_match: current.etag().expect("resource ETag").into(),
}).await?;
println!("{:?}", updated.request_id());

let mut pages = workspace.agents().pages(AgentsListOptions {
    limit: Some(20), ..Default::default()
});
while let Some(page) = pages.next().await? {
    for agent in page.data.items {
        println!("{}", agent.id);
    }
}
# Ok(())
# }
```

`Response<T>` exposes `.data`, `.status`, `.headers`, `.etag()` and `.request_id()`. Conditional mutations require a nonempty `if_match`; Memory revision restore permits an absent target. Service owns stale-precondition validation. The SDK never automatically retries uncertain mutations.

Cursor collections expose lazy `pages`; options are owned snapshots, there is no prefetch, and repeated cursors fail instead of looping. Bounded catalogues and Run Items snapshots do not invent pagination. Selectors remain typed where the contract requires integers or enums. Inline enum selectors are generated as `resources::ProviderKind`, `MemberKind` and `SkillSource`, rather than arbitrary strings:

```rust
use a13n::resources::{MemberKind, OrganizationMembersListOptions, ProviderKind, SkillSource, SkillsListOptions};
let members = OrganizationMembersListOptions { kind: Some(MemberKind::ServiceAccount), ..Default::default() };
let skills = SkillsListOptions { source: Some(SkillSource::Github), ..Default::default() };
assert_eq!(ProviderKind::Memory.as_str(), "memory");
assert_eq!(serde_json::to_string(&members.kind).unwrap(), "\"service_account\"");
assert_eq!(skills.source.unwrap().as_str(), "github");
```

## Observe a Thread

```rust,no_run
use a13n::{Client, streaming::{StreamOptions, ThreadFrame}};

# async fn example(client: &Client) -> Result<(), a13n::Error> {
let workspace = client.resources().workspaces().at("ws_example");
let thread = workspace.threads().at("thd_example");
let mut events = thread.events(StreamOptions {
    max_reconnects: 3, // explicit budget; default is no reconnection
    ..Default::default()
}).await?;
while let Some(frame) = events.next().await? {
    match frame {
        ThreadFrame::Delta { data, .. } => println!("{:?}", data.event),
        ThreadFrame::Boundary { data, .. } => println!("{}", data.run_id),
        ThreadFrame::Changed(_) => { let _current = thread.get().await?; }
        ThreadFrame::Gap(signal) | ThreadFrame::Reset(signal) => {
            let _snapshot = workspace.runs().at(signal.run_id).items().get().await?;
            // Apply the authoritative snapshot to your application state.
        }
    }
}
events.close();
# Ok(())
# }
```

Only delta/boundary frames carry cursors. Polling the next `next()` acknowledges the previously returned cursor frame; finish applying it first. `applied_cursor()` and `last_received_cursor()` are distinct. Reconnection sends only the applied cursor, and only newly acknowledged cursor progress resets its budget. Hints require authoritative readback, not a fabricated checkpoint. Durable application checkpoints are your responsibility.

Dropping a pending read future preserves partial framing and recovery delay state. Dropping or closing observation never acknowledges pending data and never cancels the durable Run. A stream has one mutable reader. Authentication and protocol failures are terminal.

## Binary data and authentication

Binary and raw SSE responses are unbuffered `BinaryResponse` values. Consume `.chunk().await?` until `None`, or drop/close the body. `thread.stream().get(...)` exposes raw SSE bytes; use `thread.events(...)` for typed frames and applied-cursor recovery. Uploads take `UploadFile` with an owned body, filename and MIME type, including a streaming `reqwest::Body`. Image operations require an explicitly allowed content type.

`Client::new` uses a Bearer token. For public requests use `Client::builder(base_url).build()`. Session clients use `.session(Arc<reqwest::cookie::Jar>, csrf_callback)` with a caller-owned jar and a concurrent-safe callback returning the current CSRF token. Bearer and session credentials cannot be combined. `.http_builder(...)` configures TLS and connection settings; SDK redirect and no-automatic-retry policy remains in force. There is no whole-response timeout; use Tokio timeout or drop the whole operation future.

Ordinary JSON and error bodies have a 16 MiB default bound, configurable by `.response_limit(...)`. `Error::Api` exposes HTTP status, Service code/message/details, request ID, retry advice and headers. `Error::Transport(TransportError)` reports build/request/body stage and a safe transport category. `Error::Protocol(ProtocolError)` reports the violated response boundary, HTTP status and request ID when available. Their `Display`, `Debug` and error sources retain no raw transport cause, URL or response body; protocol request IDs are explicitly readable but not formatted. `Connect` covers connection, DNS and TLS failures without claiming to distinguish them. The outer Tokio timeout remains separate from an SDK error. `close()` cancels local requests, waits and body delivery and releases the client pool. Cancellation does not prove server rollback or stop a Run; use the explicit interrupt operation for that.

### Handle errors explicitly

```rust,no_run
use a13n::{Client, Error};

# async fn example(client: &Client) -> Result<(), Error> {
let agent = client.resources().workspaces().at("ws_example").agents().at("agt_example");
match agent.get().await {
    Ok(response) => println!("Agent {}", response.data.id),
    Err(Error::Api(api)) => {
        // Inspect status/code for authorization, stale ETag or other Service failures.
        // Re-read/reconcile before deciding whether a mutation may be retried.
        eprintln!("HTTP {}; code={}", api.status, api.code);
    }
    Err(Error::Transport(error)) => eprintln!("{:?}: {:?}", error.stage, error.kind),
    Err(Error::Protocol(error)) => eprintln!("{:?}; HTTP {:?}", error.kind, error.status),
    Err(Error::Closed) => eprintln!("Client was closed locally"),
    Err(Error::InvalidInput) => eprintln!("Check URL, IDs, MIME type and required preconditions"),
}
# Ok(())
# }
```

The examples use `?` to surface errors, not to imply a retry. HTTP acceptance and a successful wait operation still require inspecting the returned business state. README Service examples are compile-checked Rustdoc examples; they are not run against a live Service during `cargo test`.

## Advanced generated HTTP access

```rust,no_run
use a13n::{Client, generated::apis::default_api};

# async fn example(client: &Client) -> Result<(), Box<dyn std::error::Error>> {
let response = client.execute(async |api| default_api::health_healthz_get(api).await).await?;
println!("{}", response.status);
# Ok(())
# }
```

`execute` borrows a generated configuration sharing the client's pool, credentials and shutdown. Generated models preserve unions, nullable presence and extensible Run status strings. Generated errors retain typed entities and raw content. This advanced parser is not subject to the ordinary JSON bound. Consume low-level raw response bodies inside the closure when parent shutdown must cover delivery. Standalone generated configurations have independent ownership.

## Development

Use Rust 1.97.0 with rustfmt/Clippy, Python 3.13, uv and Make. No Service checkout or sibling SDK is needed for ordinary development.

```bash
make install
make generate         # pinned local inputs; uv supplies the generator's JDK
make check-all        # SDK tests/package and separate CLI checks/tests/build
make cli-check-all
```

OpenAPI Generator 7.25.0 and repository-owned Apache-2.0 templates generate low-level bindings. A static resource generator owns the ordinary graph and per-operation dispatch tests. Fix generators, not generated output; commit regenerated output with input changes. Generation tests exercise hypothetical future operations, not just the current inventory.

See [contract provenance](contract/README.md), [SDK and CLI contracts](spec/README.md) and [Contributing](CONTRIBUTING.md). Local tests alone do not establish live Service compatibility or publication.

## License

Licensed under the Apache License 2.0.
