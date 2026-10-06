# Create an Agent and continue a conversation

An Agent is a reusable configuration; a Thread groups messages, but does not permanently belong to one Agent. If you already have an Agent ID, skip creation and go straight to `client.agent(id)`.

## Prerequisites

Follow the [source installation](../README.md#get-started-from-source). Supply `A13N_SERVICE_URL`, `A13N_API_TOKEN`, an existing `A13N_MODEL_KEY` that your Service can use, and **two different** request keys (`A13N_FIRST_KEY` and `A13N_FOLLOWUP_KEY`). Agent creation changes Service state; use a workspace where you are allowed to create an Agent. A Model is selected by **key**; the created Agent is addressed by **ID**. Your Service must also have the provider credentials for that Model.

## Create, start and send

Add `serde_json = "1"` to the sibling application and replace `src/main.rs` with this complete example:

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
            configuration: Some(Some(Box::new(models::RunConfigurationInput {
                allowed_hosts: Some(None), // Explicitly unrestricted for this new Run.
                extensions: Some(std::collections::HashMap::from([
                    ("example.workflow".into(), serde_json::json!({"enabled": false, "limit": 0, "tags": []})),
                ])),
            }))),
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

## Bring completed conversation context into a new Thread

If an application already has completed Pydantic AI model messages, `StartOptions.message_history` passes their JSON objects to the **new** Thread before its first ordinary payload. Add `serde_json = "1"` to the sibling app. This separate example imports a closed user/tool/model exchange; supply a Service URL, API key, existing `A13N_AGENT_ID` and fresh `A13N_IMPORT_KEY`:

```rust
use a13n::{Client, Secret, StartOptions};
use std::{collections::HashMap, env, error::Error};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let url = env::var("A13N_SERVICE_URL")?;
    let client = Client::new(&url, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let history: Vec<HashMap<String, serde_json::Value>> = serde_json::from_value(serde_json::json!([
        {"kind":"request", "parts":[{"part_kind":"user-prompt", "content":"Is A-100 available?"}]},
        {"kind":"response", "parts":[{"part_kind":"tool-call", "tool_name":"lookup_inventory",
            "tool_call_id":"previous-call-1", "args":{"sku":"A-100"}}]},
        {"kind":"request", "parts":[{"part_kind":"tool-return", "tool_name":"lookup_inventory",
            "tool_call_id":"previous-call-1", "content":{"quantity":3}}]},
        {"kind":"response", "parts":[{"part_kind":"text", "content":"Three are available."}]}
    ]))?;
    let mut interaction = client.agent(env::var("A13N_AGENT_ID")?).start_with(
        "Continue from that exchange", env::var("A13N_IMPORT_KEY")?,
        StartOptions { message_history: Some(history.clone()), ..Default::default() },
    ).await?;
    let thread = interaction.thread.resource().get(Default::default()).await?;
    assert_eq!(thread.data.message_history, history);
    println!("Thread {}, exact Run status: {:?}", thread.data.id, interaction.result().await?.status());
    Ok(())
}
```

`cargo +1.97.0 check` compiles it; a real run prints the Thread ID and exact Run status, while Thread readback retains the submitted import. This is **native Pydantic AI ModelMessage JSON**, not UI display Items, OpenAI chat messages, a serialized Harness checkpoint, or an instruction to execute historical tools. The Service validates completed text/tool-result pairs, at most 256 messages and 262144 UTF-8 bytes of normalized submitted JSON; it rejects media, system instructions and unresolved calls before creating any Thread. The SDK intentionally does not duplicate those validation rules. A later `agent.send` supplies only its new payload, never `message_history` again; an initial failed Run may still start a later root Run from the same immutable import, while a fork inherits the existing checkpoint.

## Structured messages and per-Run options

The printed Items are the recent committed display window, not every earlier Item. `complete` means sealed, not all history loaded; use dense Item ordinals and explicit `before`/`after` reads as in [streams and results](streams-and-results.md). `start()` and `send()` accept either a string (one text part) or a generated `models::MessagePayload`. See [files and Memory](files-and-memory.md) for an Asset part. `start_with(input, key, StartOptions)` and `send_with(thread_id, input, key, SendOptions)` expose typed revision choice, delivery and `RunOptionsInput` overrides; `StartOptions` also accepts a Service Session ID, environment and Memory mounts, and MCP headers. These options do not assign an Agent owner to the Thread.

`RunOptionsInput.configuration` is independent of Agent overrides: `None` omits it, `Some(None)` sends null and `Some(Some(Box::new(...)))` sends its complete snapshot. On a new Run omission/null selects defaults; steering an active (accepted or running) Run with omission/null retains its frozen value. An explicit `{}` does not merge defaults, `allowed_hosts: null` clears restrictions and `[]` denies every destination. Arbitrary namespaced `extensions` retain nested JSON, false, zero and empty values. A different explicit steering snapshot against an active (accepted or running) Run produces Service `409` with reason `run_configuration_immutable`; the SDK neither retries nor silently switches delivery. A waiting Run is sealed, not active: ordinary messages queue until explicit resume and do not trigger this active-Run configuration comparison. Choose `SendOptions.delivery = Some(models::Delivery::NextRun)` explicitly for a later Run with new configuration. Recovery, resume and children retain their accepted snapshot. Inspect `outcome.snapshot.data.options.configuration` for authoritative readback; normalization and media host/TLS enforcement belong to Service/Harness.

The example puts `models::AgentOverrideInput.instructions` inside `RunOptionsInput.overrides` and `StartOptions.options` for the first Run only. Use the same shape in `SendOptions.options` if a follow-up also needs an override. The Service owns the merge: Run settings replace Agent settings before Model defaults are overlaid. Each `extra_body` or `extra_headers` object replaces the inherited object, and `{}` clears it. Do not approximate Service routing or provider policy in your client.

**Common mistakes:** An Agent name is not an Agent ID, and only Models use keys. Do not reuse the first request key for a *different* follow-up. A `waiting`, `failed` or `cancelled` outcome is not a successful reply; inspect it before sending dependent work. Use [waiting and resume](waiting-and-resume.md) for pending human or client-tool actions.
