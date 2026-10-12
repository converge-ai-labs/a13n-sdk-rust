# Start and continue a conversation

An Agent is reusable configuration; a Thread groups messages but has no permanent Agent owner. Use an existing Agent **ID** and the [source-installed SDK](../README.md#get-started-from-source). Add `serde_json = "1"` to your application. Supply `A13N_SERVICE_URL`, `A13N_API_TOKEN` and `A13N_AGENT_ID`.

## Structured input, options and follow-up

```rust
use a13n::{Client, Input, InputPart, Overrides, RunConfiguration, RunOptions, RunStatus, Secret, StartOptions};
use std::{collections::HashMap, env, error::Error};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let client = Client::new(&env::var("A13N_SERVICE_URL")?, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let agent = client.agent(env::var("A13N_AGENT_ID")?);
    let input = Input::parts([
        InputPart::text("Summarize this project"),
        InputPart::json(serde_json::json!({"priorities":["quality","cost"],"enabled":false})),
    ]);
    let options = StartOptions {
        run_options: Some(RunOptions::new()
            .overrides(Some(Overrides::new().instructions(Some("Be concise.".into()))))
            .configuration(Some(RunConfiguration::new().allowed_hosts(None)
                .extensions(HashMap::from([("example.workflow".into(), serde_json::json!({"limit":0,"tags":[]}))]))))),
        ..Default::default()
    };
    let mut first = agent.start_with(input, options).await?;
    println!("Save identity for recovery: {:?}", first.identity);
    let outcome = first.result().await?;
    println!("Exact Run {}: {:?}, output {}", outcome.run_id, outcome.status, outcome.output);
    if outcome.status == RunStatus::Completed {
        let mut follow_up = agent.send(&outcome.thread_id, "Explain the trade-off").await?;
        let second = follow_up.result().await?;
        println!("Follow-up {:?}: {}", second.status, second.output);
    }
    Ok(())
}
```

`cargo +1.97.0 check` compiles without a Service. Each ordinary start/send chooses one random key; `StartOptions.idempotency_key` or `SendOptions.idempotency_key` accepts your pre-persisted key for durable recovery. No POST is automatically replayed. Waiting, failed and cancelled are explicit outcomes, not successful replies. Save Thread/Entry IDs, not a latest Run pointer; a queued Entry binds its authoritative Run only after **consumption**, never assignment.

`Input::parts` preserves ordering and owns text, JSON, URL and Asset discriminators. `InputPart::json(serde_json::Value::Null)` is an explicit null value. `InputPart::url(url)` references media without an SDK-specific media-type field; `InputPart::asset(id)` references an already-published Asset. Service/Harness owns acquisition, media validation, TLS and byte budgets; see [files and Memory](files-and-memory.md).

## Every valid Run option is public

`RunOptions` exposes configuration, overrides, labels and max_usage as thin canonical-wire-backed builders. `RunConfiguration` exposes allowed_hosts and arbitrary namespaced extensions. `Overrides` exposes **all** current fields: client_tools, connection_tools, instructions, media_understanding, model, model_characteristics, model_settings, output_spec, plugins, retries, reviewer, skills, subagents and toolsets. Nested canonical types such as `OutputSpec`, `UsageLimit`, `ClientToolDefinition` and `RetryOverride` are root exports. Their permission/reviewer enums are also public (`ClientToolPermission`, `ToolPermissionSetting`, `ReviewerOnError`, `ReviewerOnFlagged`, `RuleOnFlagged`, `ToolRiskLevel`), as are `MemoryAccess` and `ToolReviewRule`. Arbitrary settings and subagent maps remain native JSON, not stringified dictionaries. Advanced `from_wire`/`into_wire` is explicit and not required to use a supported option.

For nullable builders, not calling the method omits the field; passing `None` sends null; passing `Some(value)` sends that exact value. Empty maps/arrays/strings, false and zero remain values. Configuration is a complete snapshot, not a local merge: omission/null selects defaults for new Runs and retains an active Run's frozen snapshot when steering. `.configuration(Some(RunConfiguration::new()))` sends `{}`; `.allowed_hosts(None)` is unrestricted and `.allowed_hosts(Some(vec![]))` denies all destinations. A differing explicit active-Run snapshot returns Service `409` with reason `run_configuration_immutable`; the SDK never retries or silently switches delivery. Waiting is sealed, not active. Explicit `SendOptions.delivery = Some(a13n::Delivery::NextRun)` chooses a later Run; recovery/resume/children retain their inherited snapshot. Read `outcome.http_response.data.options.configuration` for authoritative readback.

`StartOptions` also exposes revision, delivery, environment/Memory mounts, Session ID, MCP headers and imported history; `SendOptions` exposes revision, delivery and grouped Run options. Service owns Agent/Model defaults, settings replacement, host normalization and provider routing. No option makes an Agent the permanent Thread owner.

## Fork from an exact Run

`outcome.run.fork(agent_id, Input::parts(parts), a13n::ForkOptions::default()).await?` returns the same finite Interaction for a **new** Thread/Entry. Plain text also works. `ForkOptions` exposes the chosen key, observation settings, revision, delivery, environments, fresh_environments, Memory mounts and grouped Run options. Save its identity and call result/next_text just as for start; no Agent fork alias, latest-pointer lookup or automatic fork occurs.

## Import completed native context once

A new Thread can import completed Pydantic AI ModelMessage JSON before its first ordinary payload. This separate example needs the same endpoint/token/Agent ID plus `A13N_IMPORT_KEY`, which you persist before sending:

```rust
use a13n::{Client, Secret, StartOptions};
use std::{collections::HashMap, env, error::Error};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let client = Client::new(&env::var("A13N_SERVICE_URL")?, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let history: Vec<HashMap<String, serde_json::Value>> = serde_json::from_value(serde_json::json!([
        {"kind":"request","parts":[{"part_kind":"user-prompt","content":"Is A-100 available?"}]},
        {"kind":"response","parts":[{"part_kind":"tool-call","tool_name":"lookup_inventory","tool_call_id":"previous-call-1","args":{"sku":"A-100"}}]},
        {"kind":"request","parts":[{"part_kind":"tool-return","tool_name":"lookup_inventory","tool_call_id":"previous-call-1","content":{"quantity":3}}]},
        {"kind":"response","parts":[{"part_kind":"text","content":"Three are available."}]}
    ]))?;
    let mut interaction = client.agent(env::var("A13N_AGENT_ID")?).start_with(
        "Continue from that exchange", StartOptions {
            idempotency_key: Some(env::var("A13N_IMPORT_KEY")?), message_history: Some(history.clone()), ..Default::default()
        },
    ).await?;
    let thread = interaction.thread.as_ref().ok_or("Missing Thread")?.resource().get(Default::default()).await?;
    assert_eq!(thread.data.message_history, history);
    println!("Thread {}, exact Run status: {:?}", thread.data.id, interaction.result().await?.status);
    Ok(())
}
```

This is native Pydantic AI history, **not** UI Items, OpenAI chat messages, a Harness checkpoint or authorization to execute historical tools. Service validates completed tool/result pairing, at most 256 messages and 262144 normalized UTF-8 bytes; it rejects media, system instructions and unresolved calls before creating a Thread. Follow-up sends never repeat message_history. The accepted import remains immutable in Thread readback; an initial failed Run can still start a later root Run from it, while a fork inherits checkpoint context.

## Advanced setup: create an Agent

Agent management remains a complete generated resource workflow, not another high-level invocation API. Use a workspace authorized to create an Agent and an existing **Model key** with configured provider credentials. This standalone setup example deliberately uses canonical wire models:

```rust
use a13n::{Client, Secret, generated::models};
use std::{env, error::Error};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let client = Client::new(&env::var("A13N_SERVICE_URL")?, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let created = client.api().agents().create(
        &models::AgentCreate::new(models::AgentConfigInput::new(env::var("A13N_MODEL_KEY")?), "Project explainer".into()),
        Default::default(),
    ).await?;
    println!("Agent ID: {}", created.data.id);
    Ok(())
}
```

Compile first; running creates Service state. Agent names are not IDs, and only Models use keys. Continue pending actions through [waiting and resume](waiting-and-resume.md), not a normal message that bypasses a wait.
