# Execution settings and Service compatibility

## Set Thread defaults and one Run override

The following fragment assumes an authenticated client and an existing Agent. The top-level execution field sets the new Thread's defaults; `options.execution` changes only the submitted Run.

```rust
use a13n::{
    Client, ExecutionOverride, RunOptions, Secret, StartOptions, Steering, ToolExecutionMode,
    ToolExecutionOverride,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new(
        &std::env::var("A13N_SERVICE_URL")?,
        Secret::new(std::env::var("A13N_API_TOKEN")?),
    )?;
    let agent_id = std::env::var("A13N_AGENT_ID")?;
    let execution = ExecutionOverride {
        environment: Some(Some(
            [
                ("APP_ENV".into(), "staging".into()),
                ("EMPTY".into(), "".into()),
            ]
            .into(),
        )),
        ..Default::default()
    };
    let options = RunOptions::new().execution(Some(ExecutionOverride {
        environment: Some(Some([("APP_ENV".into(), "test".into())].into())),
        tools: Some(Some(Box::new(ToolExecutionOverride {
            mode: Some(Some(ToolExecutionMode::Auto)),
            foreground_wait_seconds: Some(Some(0.0)),
        }))),
        steering: Some(Some(Steering::Boundary)),
    }));
    let mut interaction = client
        .agent(agent_id)
        .start_with(
            "Run the checks",
            StartOptions {
                execution: Some(Some(execution)),
                run_options: Some(options),
                ..Default::default()
            },
        )
        .await?;
    interaction.close();
    Ok(())
}
```

For an existing Thread use `send_with` and `SendOptions.run_options`. Persistent defaults use `client.api().threads().at(thread_id).update(...)` with `models::ThreadUpdate.execution` and an explicit If-Match ETag. Saved Agent configuration uses `models::AgentConfigInput.execution` through the revision API.

For nullable optional fields, `None` omits, `Some(None)` sends null, and `Some(Some(value))` sends a value. Generated boxed fields require `Box::new`; builders accept the unboxed canonical value. No values are merged locally.

## Inheritance belongs to Service

For each environment variable the order is recipe → Agent revision → Thread → Run → explicit command. A later layer replaces only the same key. For example, recipe `{A: "1", B: "recipe"}`, Thread `{B: "thread"}` and Run `{A: "2"}` resolve to `{A: "2", B: "thread"}` before command-specific values.

SDKs only serialize choices. Service freezes the Agent/Thread/Run overlay at acceptance; the mount supplies its frozen recipe at command dispatch. Neither SDK nor Service changes shared sandbox global variables for a Run. These are ordinary readable configuration fields, not secret storage.

On Thread PATCH, an omitted field retains its existing setting. A supplied environment map replaces that Thread map in full; null or `{}` clears that layer, exposing lower defaults. An empty string is an explicit empty value, not removal. `execution: null` clears all Thread execution overrides. A per-Run null map inherits; it does not erase lower layers. Existing accepted Runs retain their frozen settings. A conflicting override for an active Run's steer is validated by Service; the SDK does not silently retry or create another Run.

## Request context, Apps and usage migration

`RunOptions::context(Some(RequestContext { headers: Some(headers) }))` sends explicit context headers in the request body. `.context(None)` sends null; not calling the builder omits the field. Transport authentication headers are never copied. Service freezes accepted context and preserves it through recovery, resume and child Runs. CLI Agent helpers accept the same context under `--options`.

`UsageSummary.models` covers the filtered scope; use `client.api().usage().runs()` for paginated Run breakdowns instead of the removed `runs` field. The generated SDK and CLI also include built-in Skill catalogs/details/ZIP, plugins, Session Thread navigation, member usage and four conditional provider deletes. App open/resource/tool requests are explicit. Session `AppReady` follows committed Items, can repeat, and has no cursor or delta sequence.

Thread EOF inside a line, frame or UTF-8 code point uses bounded recovery from the last applied cursor. Invalid complete UTF-8/JSON remains fatal. Session connections retain caller-owned reconnect.

## Additional snapshot change

Span-content reads now locate the requested span directly. The native span-content operation no longer takes a search cursor or exposes cursor-page iteration; continue text fragments with `version` and `next_offset` instead.

## Compatibility inherited from the prerequisite update

- Native detail endpoints can return detailed records, compact summaries or text fragments. Inspect the returned variant in native API code. High-level execution observers explicitly request detailed records and retain their complete-result semantics.
- Run lists contain bounded discovery metadata (`RunListItem`), not full execution records. Read an individual Run for complete inputs, results and execution settings. Thread history includes both cursor directions and an optional anchor.
- Cursor helpers retain filters and response metadata for supported paginated results, including compact/detailed unions. Ordinal Run items remain a separate API; text fragments use offsets, not cursor helpers.
- Session streams include typed `schedule` and `occurrence` events alongside `thread`, `items`, `delta` and `gap`. Update exhaustive switches. Session frames still carry no Thread reconnect cursor.
- Tool discovery now uses proxy `ToolSearchConfig`. Retired native-mode/strategy types and the `platform_tools` override have been removed. Select tools via the current `toolsets` contract and built-in instructions via `builtin_skills`; Service owns authorization.
- Newly exposed native resources include schedules, devices, metrics data sources and dashboards. This sync does not add SDK-owned scheduling, retries or telemetry collection.

The contract snapshot is pinned to merged Service commit [`b4c9b1345752`](https://github.com/converge-ai-labs/agent-foundation/commit/b4c9b1345752bbaef3a8b076475112535357fbc7); [source metadata](../contract/source.json) records every consumed input.
