# Create an Agent and continue a conversation

An Agent is a reusable configuration; a Thread groups messages, but does not permanently belong to one Agent. If you already have an Agent ID, skip creation and go straight to `client.agent(id)`.

## Prerequisites

Follow the [source installation](../README.md#get-started-from-source). Supply `A13N_SERVICE_URL`, `A13N_API_TOKEN`, an existing `A13N_MODEL_KEY` that your Service can use, and **two different** request keys (`A13N_FIRST_KEY` and `A13N_FOLLOWUP_KEY`). Agent creation changes Service state; use a workspace where you are allowed to create an Agent. A Model is selected by **key**; the created Agent is addressed by **ID**. Your Service must also have the provider credentials for that Model.

## Create, start and send

Replace `src/main.rs` in the sibling application with this complete example:

```rust
use a13n::{Client, Secret, StartOptions, generated::models};
use std::{env, error::Error};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let url = env::var("A13N_SERVICE_URL")?;
    let client = Client::new(&url, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let config = models::AgentConfigInput::new(env::var("A13N_MODEL_KEY")?);
    let created = client.resources().agents()
        .create(&models::AgentCreate::new(config, "Project explainer".into()), Default::default())
        .await?;
    let agent = client.agent(created.data.id.clone());

    let overrides = models::AgentOverrideInput {
        instructions: Some(Some("Summarize concisely and identify open questions.".into())),
        ..Default::default()
    };
    let options = StartOptions {
        options: Some(Box::new(models::RunOptionsInput {
            overrides: Some(Some(Box::new(overrides))),
            ..Default::default()
        })),
        ..Default::default()
    };
    let mut first = agent.start_with("Summarize this project", env::var("A13N_FIRST_KEY")?, options).await?;
    let thread_id = first.thread.id.clone();
    let first_outcome = first.result().await?;
    println!("Thread: {thread_id}, first Run: {:?}", first_outcome.status());

    // Only continue automatically when the first Run actually completed.
    if *first_outcome.status() == models::RunStatus::Completed {
        let mut follow_up = agent.send(&thread_id, "Explain the trade-off", env::var("A13N_FOLLOWUP_KEY")?).await?;
        let second = follow_up.result().await?;
        println!("Follow-up: {:?}", second.status());
        if *second.status() == models::RunStatus::Completed {
            let items = second.run.items().get(Default::default()).await?;
            println!("Committed items: {:#?}", items.data.items);
        }
    }
    Ok(())
}
```

Run `cargo +1.97.0 check`, then `cargo +1.97.0 run` with real credentials, Model key and fresh request keys. The program uses typed options to override the first Run's instructions, then prints the returned Thread ID, both **exact** Run statuses, and committed items after a completed follow-up. Save the Thread ID for later processes; after restart you can call `client.agent(agent_id).send(thread_id, ...)` with an explicitly chosen Agent. A queued first message may not yet have a Run; `result()` waits for its Entry to be **consumed** before observing that Run.

## Structured messages and per-Run options

The printed Items are the retained committed display, not guaranteed to cover every earlier item if the display limit dropped some; inspect `complete` and `dropped` as in [streams and results](streams-and-results.md). `start()` and `send()` accept either a string (one text part) or a generated `models::MessagePayload`. See [files and Memory](files-and-memory.md) for an Asset part. `start_with(input, key, StartOptions)` and `send_with(thread_id, input, key, SendOptions)` expose typed revision choice, delivery and `RunOptionsInput` overrides; `StartOptions` also accepts a Service Session ID, environment and Memory mounts, and MCP headers. These options do not assign an Agent owner to the Thread.

The example puts `models::AgentOverrideInput.instructions` inside `RunOptionsInput.overrides` and `StartOptions.options` for the first Run only. Use the same shape in `SendOptions.options` if a follow-up also needs an override. The Service owns the merge: Run settings replace Agent settings before Model defaults are overlaid. Each `extra_body` or `extra_headers` object replaces the inherited object, and `{}` clears it. Do not approximate Service routing or provider policy in your client.

**Common mistakes:** An Agent name is not an Agent ID, and only Models use keys. Do not reuse the first request key for a *different* follow-up. A `waiting`, `failed` or `cancelled` outcome is not a successful reply; inspect it before sending dependent work. Use [waiting and resume](waiting-and-resume.md) for pending human or client-tool actions.
