# Continue a waiting Run

A Run can pause for a client tool, a human approval or a question. Inspect its **exact pending set** before taking action. `resume` accepts structured answers for client tools and approvals; a `user_input` question is answered with an ordinary message, not a structured answer.

Start with the sibling application from the [quick start](../README.md#get-started-from-source), and add `serde_json = "1"` under `[dependencies]`. Set `A13N_SERVICE_URL`, `A13N_API_TOKEN` and an existing `A13N_WAITING_RUN_ID`. For the demonstration client tool, configure an Agent tool named `lookup_inventory` with a string `sku` argument and make an `inventory.json` file in the application's directory, for example `printf '{"A-100":3}\n' > inventory.json`. Replace this file lookup with your real tool implementation in production. Provide a fresh `A13N_RESUME_KEY` for a structured answer, or `A13N_MESSAGE_KEY` for a question reply. An authorized person must review the printed request and type the approval decision interactively.

```rust
use a13n::{Client, Secret, generated::models};
use std::{env, error::Error, io};

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
    if pending.items.len() != 1 {
        return Err("This example handles exactly one pending action; inspect the whole batch".into());
    }
    let item = &pending.items[0];
    println!("Requested {:?}: {} {:#?}", item.kind, item.tool_name, item.arguments);

    if item.kind == models::PendingKind::UserInput {
        println!("Ask the person the question shown above, then type their reply:");
        let mut reply = String::new();
        io::stdin().read_line(&mut reply)?;
        if reply.trim().is_empty() { return Err("An empty reply is not a decision".into()); }
        let mut interaction = client.agent(&waiting.snapshot.data.agent_id)
            .send(&waiting.snapshot.data.thread_id, reply.trim(), env::var("A13N_MESSAGE_KEY")?)
            .await?;
        println!("Question reply: Entry {}", interaction.receipt.data.entry.id);
        println!("Successor status: {:?}", interaction.result().await?.status());
        return Ok(());
    }

    let answer = if item.kind == models::PendingKind::ClientTool {
        if item.tool_name != "lookup_inventory" {
            return Err("Unknown client tool; do not invent a result".into());
        }
        let sku = item.arguments.get("sku").and_then(serde_json::Value::as_str)
            .ok_or("lookup_inventory needs a string sku")?;
        let inventory: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string("inventory.json")?)?;
        let quantity = inventory.get(sku).and_then(serde_json::Value::as_u64)
            .ok_or("SKU not present in inventory data")?;
        let result = serde_json::json!({"sku": sku, "quantity": quantity});
        models::Answer::Complete(Box::new(models::Complete::new(
            models::complete::Action::Complete, Some(result), item.tool_call_id.clone(),
        )))
    } else {
        // The only other pending kind is approval. Never approve on behalf of a person.
        println!("Reviewer: type approve or reject after checking the request:");
        let mut decision = String::new();
        io::stdin().read_line(&mut decision)?;
        match decision.trim() {
            "approve" => models::Answer::Approve(Box::new(models::Approve::new(
                models::approve::Action::Approve, item.tool_call_id.clone(),
            ))),
            "reject" => {
                println!("Reason for rejection:");
                let mut reason = String::new();
                io::stdin().read_line(&mut reason)?;
                if reason.trim().is_empty() { return Err("A rejection reason is required here".into()); }
                let mut rejection = models::Reject::new(
                    models::reject::Action::Reject, item.tool_call_id.clone(),
                );
                rejection.reason = Some(Some(reason.trim().into()));
                models::Answer::Reject(Box::new(rejection))
            }
            _ => return Err("No explicit approval decision; Run remains waiting".into()),
        }
    };
    let request = models::ResumeRequest { answers: Some(vec![answer]) };
    let successor = waiting.run.resume(&request, env::var("A13N_RESUME_KEY")?).await?;
    println!("Successor Run: {} (HTTP {})", successor.run.id, successor.receipt.status);
    let result = successor.run.wait().await?;
    println!("Successor status: {:?}", result.status());
    Ok(())
}
```

Check with `cargo +1.97.0 check`, then run with an actual waiting Run and valid credentials. The program prints the pending kind, name and arguments, then the exact successor status. The inventory example refuses unknown tools, invalid arguments and missing SKUs rather than fabricating output. In your application, validate the tool's full argument contract and execute the **requested** operation before returning its result. For an approval, obtain an actual person's decision; for a question-only wait, the ordinary message closes the question with no-response and starts its successor. A resume is not a completion guarantee: inspect its distinct successor Run and committed Items. Waiting on the original still returns `waiting`.

**Multiple actions:** this deliberately stops rather than answering an arbitrary first item. A resume normalizes the **entire** pending set: an omitted approval becomes a rejection ("No decision was given"), while an omitted client result or question becomes `no_response`. A `user_input` item never accepts a structured answer; ordinary messages cannot approve, reject or complete an approval or client tool. Build an explicit answer batch only for the actions your application or reviewers really handled.

## If your message is still queued

`agent.start(...)` and `agent.send(thread_id, ...)` return an `Interaction` immediately with `interaction.receipt` and `interaction.thread`. The receipt's `run` may be `null` while the Entry waits behind an approval. `interaction.result().await?` waits for **consumption**, not provisional assignment: an assigned Entry can return to pending. If you only have IDs after restarting, inspect `client.entry(thread_id, entry_id).resource().get(Default::default()).await?`; `Entry::wait(interval)` stops at consumed, failed or withdrawn. Only the consumed Entry's `assigned_run_id` names the incorporating Run. `Submitted::bind(&client, raw_receipt)?.wait()` offers the same consumed-Entry/exact-Run behavior for advanced raw submissions. Failed/withdrawn Entries yield `Error::Submission` with an Entry snapshot rather than an unrelated Run result.

The default Interaction observation budget is 300 seconds total across Entry and Run polling (500 ms interval), including in-flight reads. It does not renew when queueing ends. `Entry::wait(interval)` itself has no implicit deadline: wrap it in your own timeout when needed. A timeout or dropped future ends **local** observation; it does not approve, interrupt, resubmit or undo remote work. Save the original request key and receipt IDs for [readback and recovery](errors-and-recovery.md).
