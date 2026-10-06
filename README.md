# a13n Rust SDK

Use the async Rust SDK to submit work to an existing a13n Service Agent and read its result. The SDK and companion [CLI](a13n-service-cli/README.md) are in pre-public source development; version `0.0.0` is not a published crate release.

## Get started from source

You need a running Service URL, a workspace API key authorized for an existing Agent, and that Agent's **ID** (not its name). An API key already selects its workspace. Do not supply a workspace ID for an ordinary Agent call. Use a new caller-owned idempotency key for each *different* submission; reuse one only when reconciling the *same* uncertain submission.

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
use a13n::{Client, Secret, generated::models};
use std::{env, error::Error};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let service_url = env::var("A13N_SERVICE_URL")?;
    let client = Client::new(
        &service_url,
        Secret::new(env::var("A13N_API_TOKEN")?),
    )?;
    let agent = client.agent(env::var("A13N_AGENT_ID")?);
    let mut interaction = agent.start("Summarize this project", env::var("A13N_REQUEST_KEY")?).await?;
    println!("Thread: {}", interaction.thread.id);

    let outcome = interaction.result().await?; // No SSE connection is needed.
    match outcome.status() {
        models::RunStatus::Completed => {
            let messages = outcome.run.items().get(Default::default()).await?;
            println!("Messages: {:#?}", messages.data.items);
        }
        models::RunStatus::Waiting => println!("Pending action: {:?}", outcome.pending()),
        models::RunStatus::Failed | models::RunStatus::Cancelled => println!("Failure: {:?}", outcome.failure()),
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
export A13N_REQUEST_KEY='a-unique-key-for-this-submission'
cargo +1.97.0 run
```

The example prints the recent committed message/tool window, not an automatically loaded lifetime transcript. `complete` means the Run is sealed; dense Item ordinals and explicit `before`/`after` reads expose earlier history. Default Items reads have `baseline=true` and include the whole mutable tail even beyond `limit`; historical windows cannot seed live coverage. `outcome.output()` is the Run's optional output value, not necessarily its conversation text.

`start()` returns an **Interaction**; `result()` waits for that message to finish or pause. Check the status: `waiting` needs action, while `failed` and `cancelled` are not successful replies. Save the printed Thread ID to continue the conversation. Keep the request key if you may need to reconcile a network failure, and never put token bytes in committed files.

## Next steps

- [Create an Agent and continue](docs/agent-conversations.md) with an existing Model key, structured input and per-Run options. A Thread has no permanent Agent; choose one explicitly for each message.
- [Stream an interaction](docs/streams-and-results.md) with `next().await?`, then call `result()` and read committed Run Items. Transient frames are not a durable transcript.
- [Answer a waiting client tool](docs/waiting-and-resume.md) with a real result and observe the distinct successor Run. Queued input is not incorporated merely because it was assigned.
- [Attach files and mount Memory](docs/files-and-memory.md), [authenticate and use resources](docs/auth-and-resources.md), or [reconcile failures and timeouts](docs/errors-and-recovery.md).

The [application guide](docs/README.md) arranges these chapters by task. The complete generated resource API covers the pinned Service operations, including admin resources, pagination, binary transfers and conditional ETag updates. No automatic write retry, approval, remote interruption or successor following is hidden behind an Agent call. The [SDK contract](spec/README.md), [pinned upstream inputs](contract/README.md) and [CLI workflows](a13n-service-cli/docs/README.md) provide deeper reference.

## Develop this checkout

Use Rust 1.97.0, Python 3.13, uv and Make. `make generate` reads the locally pinned contract and regenerates SDK/CLI-owned code; `make check-all` checks code, tests, packaging, the independent extracted-crate consumer and CLI. Local checks do not publish anything or prove live Service compatibility. Licensed under Apache-2.0.
