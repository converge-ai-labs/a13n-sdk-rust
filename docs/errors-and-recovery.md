# Handle uncertain outcomes without duplicate work

A network timeout or dropped future tells you what the **client** observed, not whether the Service accepted or completed a mutation. Keep one idempotency key per logical POST and save its returned Thread/Entry/Run IDs. Do not automatically retry a write after an uncertain outcome.

Use the [source-installed application](../README.md#get-started-from-source) with `A13N_SERVICE_URL`, `A13N_API_TOKEN`, an existing `A13N_AGENT_ID` and a fresh `A13N_REQUEST_KEY`. This complete example deliberately reads back the exact Entry if local result observation times out:

```rust
use a13n::{Client, Error, Secret, generated::models};
use std::{env, error::Error as StdError, time::Duration};

// Follow this Entry's incorporation, never a Thread's latest Run.
async fn recover(client: &Client, thread_id: &str, entry_id: &str) -> Result<(), Box<dyn StdError>> {
    let entry = client.entry(thread_id, entry_id);
    let settled = entry.wait(Duration::from_millis(500)).await?;
    match settled.data.status {
        models::EntryStatus::Consumed => {
            let id = settled.data.assigned_run_id.as_deref()
                .ok_or("Consumed Entry has no assigned Run")?;
            let exact = client.run(id).wait().await?;
            println!("Recovered exact Run {id}: {:?}", exact.status());
        }
        status => println!("Entry ended as {status:?}; no incorporating Run to observe"),
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn StdError>> {
    let url = env::var("A13N_SERVICE_URL")?;
    let client = Client::new(&url, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let key = env::var("A13N_REQUEST_KEY")?;
    let mut interaction = client.agent(env::var("A13N_AGENT_ID")?)
        .start("Summarize the plan", &key).await?;
    let thread_id = interaction.thread.id.clone();
    let entry_id = interaction.receipt.data.entry.id.clone();
    println!("Receipt: Thread {thread_id}, Entry {entry_id}");

    match interaction.result().await {
        Ok(outcome) => println!("Exact Run {}: {:?}", outcome.run.id, outcome.status()),
        Err(Error::Timeout) => {
            // Observation stopped locally; resume from this submission's Entry.
            recover(&client, &thread_id, &entry_id).await?;
        }
        Err(Error::Submission(disposition)) => {
            println!("Entry {}: {:?}", disposition.entry_id, disposition.entry.data.status);
        }
        Err(other) => return Err(other.into()),
    }
    Ok(())
}
```

`cargo +1.97.0 check` compiles this without a Service call. At runtime it prints the acceptance IDs, then either the original exact Run status or a recovered consumed Entry's exact Run status. `Entry::wait` has no built-in deadline and may remain queued; wrap `recover` in your application's own timeout or retain its IDs to resume after restart. `Run::wait` has its own 300-second observation budget, so another timeout does not imply a failed Run. If `start()` itself fails *before you receive a receipt*, you may not have any IDs to read back; keep the original key and reconcile with Service state before deliberately retrying that **same** logical request. A different message needs a different key. Local cancellation or Client shutdown never means remote rollback.

## Interpret the typed errors

| Error                                  | What you can conclude                                                                     | Next step                                                          |
| -------------------------------------- | ----------------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| `Error::Api`                           | Service returned an HTTP error with safe status/code/details and request metadata.        | Fix permissions/input; on `412`, read current ETag and reconcile.  |
| `Error::Submission`                    | This exact Entry became failed or withdrawn; its snapshot and IDs are available.          | Inspect that Entry; do not treat a different Run as success.       |
| `Error::Timeout`                       | The local 300-second observation deadline expired across queueing, Run polling and reads. | Read the original Entry/Run IDs, if known.                         |
| `Error::Transport` / `Error::Protocol` | A request, response body or protocol failed locally.                                      | Do not assume a mutation was rejected; reconcile by key and IDs.   |
| `Error::Closed`                        | Local Client or Interaction observation was closed.                                       | Rebind the known IDs in an open Client if more readback is needed. |

`Run::wait()` returns `RunOutcome` for completed, waiting, failed and cancelled; these are statuses, **not** errors indicating that all work succeeded. `Interaction::close()` and drop release the local SSE connection only. `Run::interrupt()` is the explicit remote stop request. Resume with actual answer data and an explicit key; it creates a **new** Run, so waiting on the original Run cannot report the successor's result.

**Avoid two common mistakes:** an Entry in `assigned` is not yet `consumed` (assignment can roll back to pending), and the latest Thread Run may belong to another message. Reconcile from the saved Entry, then observe only its `assigned_run_id` after consumption. Safe error messages redact raw transport/body details; keep token bytes and untrusted request bodies out of your own logs too. For live frames and committed readback, see [streams and results](streams-and-results.md).
