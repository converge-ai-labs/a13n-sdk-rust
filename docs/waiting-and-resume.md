# Continue a waiting Run

A waiting Run exposes two categories: `pending.approvals` for explicit authorization decisions and `pending.calls` for returned or failed tool results. Client tools, built-in questions and other human-operated calls are all **calls**. A normal message submitted with `agent.send` remains queued behind a waiting head; it cannot answer a question or close the wait. Resume the **exact waiting Run** with complete ID-keyed maps, not an array of loosely matched answers.

Start with the [source-installed application](../README.md#get-started-from-source); add `serde_json = "1"` to `[dependencies]`. Supply `A13N_SERVICE_URL`, `A13N_API_TOKEN`, `A13N_WAITING_RUN_ID`, a fresh `A13N_RESUME_KEY` and `A13N_RESUME_INPUT` (ordinary text to accompany the result batch). If the request is an application tool named `lookup_inventory` with a string `sku`, provide actual inventory data, for example `printf '{"A-100":3}\n' > inventory.json`. For `ask_user_question`, supply `A13N_QUESTION_RESPONSE_JSON` in the Harness `UserQuestionAnswers` shape, such as `{"answers":{"Choose one":"A"}}` for the **actual** question text and allowed choice; the Service validates it against that pending call's arguments. Optional `A13N_ASSET_ID` must name an already-published Asset the successor may read. A reviewer must inspect a pending approval before typing its decision.

```rust
use a13n::{Client, Secret, generated::models, text_payload};
use std::{collections::HashMap, env, error::Error, io};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let url = env::var("A13N_SERVICE_URL")?;
    let client = Client::new(&url, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let waiting = client.run(env::var("A13N_WAITING_RUN_ID")?).wait().await?;
    if *waiting.status() != models::RunStatus::Waiting {
        println!("Not waiting: {:?}", waiting.status());
        return Ok(());
    }
    let pending = waiting.pending().ok_or("Waiting Run has no pending actions")?;
    if pending.approvals.len() + pending.calls.len() != 1 {
        return Err("Inspect every pending ID; this example handles exactly one".into());
    }
    let mut approvals = HashMap::new();
    let mut calls = HashMap::new();
    if let Some(item) = pending.approvals.first() {
        println!("Review approval {}: {} {:#?}", item.tool_call_id, item.tool_name, item.arguments);
        println!("Reviewer: type approve or deny:");
        let mut decision = String::new();
        io::stdin().read_line(&mut decision)?;
        let answer = match decision.trim() {
            "approve" => models::ApprovalDecision::Approve(Box::new(models::Approve::new(
                models::approve::Action::Approve,
            ))),
            "deny" => {
                println!("Reason for denial:");
                let mut reason = String::new();
                io::stdin().read_line(&mut reason)?;
                if reason.trim().is_empty() { return Err("Denial needs a reason here".into()); }
                let mut denial = models::Deny::new(models::deny::Action::Deny);
                denial.reason = Some(Some(reason.trim().into()));
                models::ApprovalDecision::Deny(Box::new(denial))
            }
            _ => return Err("No explicit decision; Run remains waiting".into()),
        };
        approvals.insert(item.tool_call_id.clone(), answer);
    } else if let Some(item) = pending.calls.first() {
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
        calls.insert(item.tool_call_id.clone(), models::CallResult::Returned(Box::new(
            models::Returned::new(models::returned::Status::Returned, Some(value)),
        )));
    }
    let mut request = models::Resume::new(approvals, calls);
    let mut input = text_payload(env::var("A13N_RESUME_INPUT")?);
    if let Ok(asset_id) = env::var("A13N_ASSET_ID") {
        input.content.push(models::Part::Asset(Box::new(models::AssetPart::new(
            asset_id, models::asset_part::Type::Asset,
        ))));
    }
    request.input = Some(Some(Box::new(input)));
    let successor = waiting.run.resume(&request, env::var("A13N_RESUME_KEY")?).await?;
    println!("Successor Run: {} (HTTP {})", successor.run.id, successor.receipt.status);
    println!("Successor status: {:?}", successor.run.wait().await?.status());
    Ok(())
}
```

`cargo +1.97.0 check` compiles the example without a Service. A real run prints the exact pending ID/name/arguments and the **distinct** successor's status. `request.input` is ordinary text (and optionally an Asset reference), submitted in the **same immutable resume intent** as the answer maps. It cannot replace a missing call result, approve an action, or answer a question by itself. The Service validates referenced Assets against inherited configuration and frozen mounts; the SDK passes their IDs through. The resume target must be the exact idle last waiting Run; completed, failed and cancelled histories continue through a new explicit normal message instead. Wait on the successor and read its recent committed Items before treating it as completed; a sealed Items window is not necessarily full history.

For a tool failure or an intentionally skipped question, put `models::CallResult::Failed(Box::new(models::Failed::new(message, models::failed::Status::Failed)))` under that pending call ID rather than inventing a successful result. For a batch of several pending IDs, inspect **both** arrays and fill both maps exactly; missing, unknown or category-mismatched IDs reject the entire request without a successor. Do not silently pick the first item or send an ordinary message to bypass a wait. A built-in question's returned value uses `{"answers": {question_text: selection_or_selections}, "response": "optional free text"}`; let the Service check the specific question/choice contract.

## If your message is still queued

`agent.start(...)` and `agent.send(thread_id, ...)` return an `Interaction` with `interaction.receipt` and `interaction.thread`. The receipt's `run` may be `null` while its Entry waits behind another Run, including a waiting one. `interaction.result().await?` waits for **consumption**, not provisional assignment: an assigned Entry can return to pending. After restart, inspect `client.entry(thread_id, entry_id).resource().get(Default::default()).await?`; `Entry::wait(interval)` stops at consumed, failed or withdrawn. Only the consumed Entry's `assigned_run_id` names its incorporating Run. `Submitted::bind(&client, raw_receipt)?.wait()` has the same consumed-Entry/exact-Run behavior. Failed/withdrawn Entries yield `Error::Submission` with their snapshot rather than an unrelated Run result.

The default Interaction observation budget is 300 seconds across Entry and Run polling (500 ms interval), including in-flight reads. `Entry::wait(interval)` has no implicit deadline: wrap it in your own timeout when needed. A timeout or dropped future ends **local** observation; it does not approve, interrupt, resubmit or undo remote work. Preserve the request key and receipt IDs for [readback and recovery](errors-and-recovery.md).
