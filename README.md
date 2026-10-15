# a13n Rust SDK

Use the async Rust SDK to submit work to an existing a13n Service Agent and read its result. The SDK and companion [CLI](a13n-service-cli/README.md) are in pre-public source development; version `0.0.0` is not a published crate release.

## Get started from source

You need a running Service URL, a workspace API key authorized for an existing Agent, and that Agent's **ID** (not its name). An API key already selects its workspace. Do not supply a workspace ID for an ordinary Agent call. Ordinary calls choose a random idempotency key once. For durable recovery, persist your own key and identical request body before sending; a different logical message needs a different key.

If you have access to this pre-public repository, clone it (or use an existing checkout) and create a sibling application. Rust 1.97.0 matches this checkout's tested toolchain:

```bash
rustup toolchain install 1.97.0
git clone https://github.com/converge-ai-labs/a13n-sdk-rust.git
cargo +1.97.0 new hello-a13n
cd hello-a13n
```

In `hello-a13n/Cargo.toml`, add these dependencies under the existing `[dependencies]` heading:

```toml
a13n = { path = "../a13n-sdk-rust" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Put this complete, **result-only** example in `src/main.rs`:

```rust,no_run
use a13n::{Client, RunStatus, Secret};
use std::{env, error::Error};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let service_url = env::var("A13N_SERVICE_URL")?;
    let client = Client::new(
        &service_url,
        Secret::new(env::var("A13N_API_TOKEN")?),
    )?;
    let agent = client.agent(env::var("A13N_AGENT_ID")?);
    let mut interaction = agent.start("Summarize this project").await?;
    println!("Identity: {:?}", interaction.identity);

    let outcome = interaction.result().await?; // No SSE connection is needed.
    match outcome.status {
        RunStatus::Completed => {
            if let Some(text) = &outcome.text { println!("{text}"); }
            else { println!("Structured output: {}", outcome.output); }
        }
        RunStatus::Waiting => println!("Pending approvals: {:?}, calls: {:?}", outcome.pending_approvals, outcome.pending_calls),
        RunStatus::Failed | RunStatus::Cancelled => println!("Failure: {:?}", outcome.failure),
        status => println!("Run status: {status:?}"),
    }
    Ok(())
}
```

Compile without contacting the Service, then configure your own endpoint, credential and Agent ID before running:

```bash
cargo +1.97.0 check
export A13N_SERVICE_URL='https://your-service.example'
export A13N_API_TOKEN='your-workspace-api-key'
export A13N_AGENT_ID='your-existing-agent-id'
cargo +1.97.0 run
```

The first reply comes directly from full Run output, with no Items request. `text` exists only for JSON string output (including the empty string); objects, arrays, booleans, numbers and null remain in `output`. Check the status: `waiting` needs action, and `failed`/`cancelled` are not successful replies. Save the Interaction identity's Thread/Entry IDs and key for recovery; use the Thread ID for follow-up. `outcome.http_response` explicitly retains HTTP evidence.

`Client` clones and authored Agent/Thread/Entry/Run handles are owned and task-safe, sharing one pool and authentication state. Dropping a clone does not invalidate retained handles; explicit `client.close()` on any clone closes all of them. Close/drop of an Interaction is local only.

## Next steps

- [Create an Agent and continue](docs/agent-conversations.md) with an existing Model key, structured input and per-Run options. A Thread has no permanent Agent; choose one explicitly for each message.
- [Stream an interaction](docs/streams-and-results.md) with `next_text().await?` (or raw `next()`), then call `result()` for authoritative output. Advanced Items windows are separate display readback.
- [Answer a waiting client tool](docs/waiting-and-resume.md) with a real result and observe the distinct successor Run. Queued input is not incorporated merely because it was assigned.
- [Attach files and mount Memory](docs/files-and-memory.md), [authenticate and use resources](docs/auth-and-resources.md), or [reconcile failures and timeouts](docs/errors-and-recovery.md).

The [application guide](docs/README.md) arranges these chapters by task. The complete generated resource API covers the pinned Service operations, including admin resources, pagination, binary transfers and conditional ETag updates. No automatic write retry, approval, remote interruption or successor following is hidden behind an Agent call. The [SDK contract](spec/README.md), [pinned upstream inputs](contract/README.md) and [CLI workflows](a13n-service-cli/docs/README.md) provide deeper reference.

## Native Service API and migration

Use `client.api().agents()` (or another collection on `client.api()`) for the complete typed native Service API. It shares this Client's transport, authentication, workspace context and shutdown; it is not another client. Keep `client.agent(agent_id)` for authored start/send workflows and finite Interaction results.

The current snapshot also exposes findings/analyses, Run content reads, Session message-author queries and the raw Session stream through this graph. Read `Item.content_refs` with `client.api().runs().at(run_id).contents().at(content_id).get(...)` explicitly; previews and memory evidence never trigger automatic loading. Advanced `SessionStream` exposes typed fresh snapshots and native events; see the [stream guide](docs/streams-and-results.md#advanced-native-session-events). Run Items now contains only `run`, `items` and `complete`; `RunView.display_position` and `ThreadView.message_history` are removed. Imported history input uses ordered `serde_json::Map` messages and order-preserving nested JSON. Preserve submitted key order when saving immutable history. Run overrides include lazy Environment preparation and explicit shared/isolated skill-sync policies; see the [option guide](docs/agent-conversations.md#every-valid-run-option-is-public).

Before 0.1, migrate `client.resources()` calls to `client.api()`; the old accessor is removed without an alias. Existing `a13n::resources` option types, resource handles and their borrowed lifetimes are unchanged. See [authentication and resource examples](docs/auth-and-resources.md) for runnable low-level calls.

## Develop this checkout

Use Rust 1.97.0, Python 3.13, uv and Make. `make generate` reads the locally pinned contract and regenerates SDK/CLI-owned code; `make check-all` checks code, tests, packaging, the independent extracted-crate consumer and CLI. Local checks do not publish anything or prove live Service compatibility. Licensed under Apache-2.0.
