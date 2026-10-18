# Continue a waiting Run

A waiting Run exposes two categories: `pending_approvals` for explicit authorization decisions and `pending_calls` for returned or failed tool results. Client tools, built-in questions and other human-operated calls are all **calls**. A normal message submitted with `agent.send` remains queued behind a waiting head; it cannot answer a question or close the wait. Resume the **exact waiting Run** with one complete ID-keyed Answer map, not an array of loosely matched answers.

Start with the [source-installed application](../README.md#get-started-from-source); add `serde_json = "1"` to `[dependencies]`. Supply `A13N_SERVICE_URL`, `A13N_API_TOKEN`, `A13N_WAITING_RUN_ID`, a fresh `A13N_RESUME_KEY` and `A13N_RESUME_INPUT` (ordinary text to accompany the result batch). If the request is an application tool named `lookup_inventory` with a string `sku`, provide actual inventory data, for example `printf '{"A-100":3}\n' > inventory.json`. For `ask_user_question`, supply `A13N_QUESTION_RESPONSE_JSON` in the Harness `UserQuestionAnswers` shape, such as `{"answers":{"Choose one":"A"}}` for the **actual** question text and allowed choice; the Service validates it against that pending call's arguments. Optional `A13N_ASSET_ID` must name an already-published Asset the successor may read. A reviewer must inspect a pending approval before typing its decision.

```rust
use a13n::{Answer, Client, Input, InputPart, ResumeOptions, RunStatus, Secret};
use std::{collections::HashMap, env, error::Error, io};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let url = env::var("A13N_SERVICE_URL")?;
    let client = Client::new(&url, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let waiting = client.run(env::var("A13N_WAITING_RUN_ID")?).result(Default::default()).await?;
    if waiting.status != RunStatus::Waiting {
        println!("Not waiting: {:?}", waiting.status);
        return Ok(());
    }
    if waiting.pending_approvals.len() + waiting.pending_calls.len() != 1 {
        return Err("Inspect every pending ID; this example handles exactly one".into());
    }
    let mut answers = HashMap::new();
    if let Some(item) = waiting.pending_approvals.first() {
        println!("Review approval {}: {} {:#?}", item.tool_call_id, item.tool_name, item.arguments);
        println!("Reviewer: type approve or deny:");
        let mut decision = String::new();
        io::stdin().read_line(&mut decision)?;
        let answer = match decision.trim() {
            "approve" => Answer::approve(),
            "deny" => {
                println!("Reason for denial:");
                let mut reason = String::new();
                io::stdin().read_line(&mut reason)?;
                if reason.trim().is_empty() { return Err("Denial needs a reason here".into()); }
                Answer::deny_with(Some(reason.trim().into()))
            }
            _ => return Err("No explicit decision; Run remains waiting".into()),
        };
        answers.insert(item.tool_call_id.clone(), answer);
    } else if let Some(item) = waiting.pending_calls.first() {
        println!("Review call {}: {} {:#?}", item.tool_call_id, item.tool_name, item.arguments);
        let value = match item.tool_name.as_str() {
            "lookup_inventory" => {
                let sku = item.arguments.get("sku").and_then(serde_json::Value::as_str)
                    .ok_or("lookup_inventory needs a string sku")?;
                let inventory: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string("inventory.json")?)?;
                let quantity = inventory.get(sku).and_then(serde_json::Value::as_u64)
                    .ok_or("SKU absent in inventory; do not invent a result")?;
                serde_json::json!({"sku": sku, "quantity": quantity})
            }
            "ask_user_question" => serde_json::from_str(&env::var("A13N_QUESTION_RESPONSE_JSON")?)?,
            _ => return Err("Unknown call: run and verify its real operation first".into()),
        };
        answers.insert(item.tool_call_id.clone(), Answer::returned(value));
    }
    let mut parts = vec![InputPart::text(env::var("A13N_RESUME_INPUT")?)];
    if let Ok(asset_id) = env::var("A13N_ASSET_ID") { parts.push(InputPart::asset(asset_id)); }
    let mut successor = waiting.run.resume_with(answers, ResumeOptions {
        idempotency_key: Some(env::var("A13N_RESUME_KEY")?),
        input: Some(Some(Input::parts(parts))), ..Default::default()
    }).await?;
    println!("Successor identity: {:?}", successor.identity);
    let result = successor.result().await?;
    println!("Successor {:?}: {}", result.status, result.output);
    Ok(())
}
```

`cargo +1.97.0 check` compiles the example without a Service. A real run prints the exact pending ID/name/arguments and the **distinct** successor's status. `ResumeOptions.input` is ordinary text (and optionally an Asset reference), submitted in the **same immutable resume intent** as the answer maps. It cannot replace a missing call result, approve an action, or answer a question by itself. The Service validates referenced Assets against inherited configuration and frozen mounts; the SDK passes their IDs through. The resume target must be the exact idle last waiting Run; completed, failed and cancelled histories continue through a new explicit normal message instead. Read the successor Interaction result and its status before treating its output as completed. It can also stream through next_text or raw next, with no fabricated Entry or latest-Run lookup.

For a tool failure or an intentionally skipped question, put `Answer::failed(message)` under that pending call ID rather than inventing a successful result. For a batch of several pending IDs, inspect **both** arrays and fill the ONE Answer map exactly; the SDK splits approvals/calls into both required wire maps in one mutation; missing, unknown or category-mismatched IDs reject the entire request without a successor. Do not silently pick the first item or send an ordinary message to bypass a wait. A built-in question's returned value uses `{"answers": {question_text: selection_or_selections}, "response": "optional free text"}`; let the Service check the specific question/choice contract.

## If your message is still queued

Submission evidence is explicit: `interaction.evidence.submission()` retains the actual receipt, whose `run` may be null or provisional. `interaction.run` is authoritative only after the exact Entry becomes consumed. An assigned Entry can return to pending. Failed/withdrawn Entries produce typed `Error::Submission`, distinct from failed/cancelled incorporated Runs.

After restart, construct `client.entry(thread_id, entry_id).observe(ObservationOptions::default())?` and call result on the same finite Interaction. Construction is local; it does not fabricate a receipt or perform I/O. Its one observation deadline spans Entry and Run, SSE and reads. Exact `client.run(id).observe(options)` and `.result(options)` use that core without following successors. The advanced disposition-only `Entry::wait(interval)` remains available but is not needed for recovery.

## Answer values and local lifetime

`Answer::returned(value)` requires an explicit serde_json::Value; null is a real result, not a missing answer. `Answer::deny()` omits reason, `deny_with(None)` sends null and `deny_with(Some(String::new()))` preserves empty string. The Service validates semantics and complete atomic coverage. The SDK never executes tools or approves on your behalf. `ResumeOptions.input` distinguishes omitted, explicit null and ordered input, and cannot replace a missing answer.

Resume returns a distinct successor-origin Interaction; waiting again on the original Run does not follow it. Close/drop only stops local I/O. The default budget is 300 seconds with 500 ms polling; it does not reset between queue, Run, raw/text iteration and result. Persist caller-owned resume key/body before sending when recovery must survive cancellation. See [errors and recovery](errors-and-recovery.md).

## Desktop-control waiting

`wait_reason=desktop_control` also ends a finite exact-Run Interaction normally. Inspect `outcome.http_response.data.wait_reason` and `pending.desktop_control_id`; both empty pending tool arrays are valid for a user desktop-only wait. Recover the same Run with `client.run(id).result(...)` after restart. Closing a reader, disconnecting a viewer or an expired input grant leaves durable Run control to Service.

The native graph exposes `runs().at(run).desktop_control().get/create`, Run `desktop_assistance().at(call).get`, and `desktop_assistance().at(id).view/control/request_control/disconnect/complete`. Opening metadata/viewing, requesting control and acquiring input authority are separate operations. `request_control` may return `preparing`; it grants no input credential. Renewals and lease reconciliation are explicit caller choices. The SDK opens no browser, renews no lease and reclaims no controller.

After a user has actually finished the authorized desktop work, complete its stored context with the exact browser-session token and epoch. `Control.session_id` and `Completion.session_id` are 20–72-character browser-session tokens, **not** Service Session IDs; epoch is nonnegative. Completion persists the outcome and releases access, but never starts a Run. Disconnect only releases this browser's access and does not complete the context. Agent-origin assistance additionally needs the precise persisted outcome and every other pending tool answer.

This complete example is limited to an already waiting **user desktop-only** Run with no tool answers. It makes two explicit mutations after inspecting that boundary. Set `A13N_DESKTOP_BROWSER_SESSION` and `A13N_DESKTOP_EPOCH` from the actual authorized control lease, and `A13N_RESUME_KEY` to a persisted fresh key. Run only after completing the real desktop action; compilation needs no Service or desktop.

```rust
use a13n::{Client, Secret, ResumeOptions, RunStatus, generated::models::{Completion, WaitReason}};
use std::{env, error::Error, collections::HashMap};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let client = Client::new(&env::var("A13N_SERVICE_URL")?, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let waiting = client.run(env::var("A13N_WAITING_RUN_ID")?).result(Default::default()).await?;
    if waiting.status != RunStatus::Waiting || waiting.http_response.data.wait_reason != Some(WaitReason::DesktopControl)
        || !waiting.pending_calls.is_empty() || !waiting.pending_approvals.is_empty() {
        return Err("This example requires a user desktop-only wait".into());
    }
    let id = waiting.http_response.data.pending.as_ref()
        .and_then(|pending| pending.desktop_control_id.as_ref()).and_then(|id| id.as_ref())
        .ok_or("Missing desktop-control context")?;
    client.api().desktop_assistance().at(id).complete(&Completion::new(
        env::var("A13N_DESKTOP_EPOCH")?.parse()?, env::var("A13N_DESKTOP_BROWSER_SESSION")?
    ), Default::default()).await?;
    let mut successor = waiting.run.resume_with(HashMap::new(), ResumeOptions {
        idempotency_key: Some(env::var("A13N_RESUME_KEY")?), ..Default::default()
    }).await?;
    println!("Explicit successor {}", successor.result().await?.run_id);
    Ok(())
}
```

The empty Answer map produces both required empty wire maps. Service validates that stored desktop completion and all checkpoint conditions permit this batch. Ordinary waiting calls/approvals still require their complete ID-keyed batch; never fabricate a tool answer for a desktop-control ID. `runs().at(run).desktop_control().get(...)` returns successful `None` when no control metadata exists, distinct from an unknown Run error.
