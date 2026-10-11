# Recover exact work without duplicate submission

An observation timeout is not a failed remote Run. A mutation transport failure is not proof of rejection. Save the Thread/Entry IDs from acceptance; for cross-process and cancellation-safe reconciliation, persist your own idempotency key and identical logical body **before** dispatch. Ordinary calls generate one random key, but a dropped future cannot return that key to you. Different messages need different keys; no write is automatically retried.

Use the [source-installed app](../README.md#get-started-from-source), add `serde_json = "1"`, and supply URL/token/Agent ID plus a pre-persisted `A13N_REQUEST_KEY`. This complete program saves accepted identity and recovers from that exact Entry with a new bounded observer:

```rust
use a13n::{Client, Error, ObservationOptions, Secret, StartOptions};
use std::{env, error::Error as StdError, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn StdError>> {
    let client = Client::new(&env::var("A13N_SERVICE_URL")?, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let key = env::var("A13N_REQUEST_KEY")?; // Application persisted this key/body before dispatch.
    let mut interaction = match client.agent(env::var("A13N_AGENT_ID")?).start_with(
        "Summarize the plan", StartOptions {
            idempotency_key: Some(key),
            observation: ObservationOptions { timeout: Duration::from_secs(5), ..Default::default() },
            ..Default::default()
        },
    ).await {
        Ok(interaction) => interaction,
        Err(Error::UnknownOutcome(unknown)) => {
            println!("Reconcile {} using key {} and known IDs {:?}/{:?}; no automatic replay",
                unknown.operation, unknown.idempotency_key, unknown.thread_id, unknown.run_id);
            return Err(unknown.into());
        }
        Err(other) => return Err(other.into()),
    };
    let thread_id = interaction.identity.thread_id.clone().ok_or("Missing Thread ID")?;
    let entry_id = interaction.identity.entry_id.clone().ok_or("Missing Entry ID")?;
    std::fs::write("accepted-identity.json", serde_json::to_vec(&serde_json::json!({
        "thread_id":thread_id,"entry_id":entry_id,"key":interaction.identity.idempotency_key,
    }))?)?;
    match interaction.result().await {
        Ok(outcome) => println!("Exact Run {}: {:?}, output {}", outcome.run_id, outcome.status, outcome.output),
        Err(Error::Timeout) => {
            // Local construction only: no POST, fake receipt or manual polling loop.
            let mut recovered = client.entry(&thread_id, &entry_id).observe(ObservationOptions {
                timeout: Duration::from_secs(60), ..Default::default()
            })?;
            let outcome = recovered.result().await?;
            println!("Recovered exact Run {}: {:?}", outcome.run_id, outcome.status);
        }
        Err(Error::Submission(disposition)) => println!("Entry {} ended as {:?}", disposition.entry_id, disposition.entry.data.status),
        Err(other) => return Err(other.into()),
    }
    Ok(())
}
```

Compile before running. Recovery from a saved Entry waits for **consumed**, not assigned: provisional assignment can revert. Failed/withdrawn dispositions are `Error::Submission`, never an unrelated latest Run. `RecoveredIdentity` evidence has no fabricated response. If you already know an exact Run ID, use `client.run(id).observe(options)?` or `.result(options).await?`; neither follows a successor. Unknown submission outcome may leave you without any IDs—retain the original key/body and reconcile Service state before deliberately replaying the same intent.

## Typed errors and explicit outcomes

| Value                                  | Meaning                                                                                                                                                                                                                                                                                   | Next step                                                                            |
| -------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| `Error::Api`                           | Canonical structured Service rejection, with status/code/details and response metadata.                                                                                                                                                                                                   | Fix permissions/input; reconcile ETags on 412.                                       |
| `Error::UnknownOutcome`                | Mutation dispatch was attempted but acknowledgement is unreliable, including malformed accepted receipts. Chosen key, operation, known IDs, actual HTTP status (when received) and original typed cause are retained. A status-only/HTML gateway response is not a trustworthy rejection. | Reconcile the identical intent; never assume rollback or generate a replacement key. |
| `Error::Submission`                    | Exact Entry failed or was withdrawn before incorporation.                                                                                                                                                                                                                                 | Inspect its retained snapshot; do not substitute another Run.                        |
| `Error::Timeout`                       | Frozen local observation deadline expired across queue, Run, SSE or readback.                                                                                                                                                                                                             | Recover saved IDs with a new bounded observer.                                       |
| `Error::MutationTimeout`               | An absolute caller request deadline expired before dispatch. After attempted dispatch it is the cause inside UnknownOutcome.                                                                                                                                                              | Use dispatch evidence, not timeout alone, to decide reconciliation.                  |
| `Error::PreviewInterrupted`            | Conservative text projection stopped.                                                                                                                                                                                                                                                     | Still-open result/raw observation remains usable; final output is authoritative.     |
| `Error::Transport` / `Error::Protocol` | Local transport/protocol failure; high-level uncertain mutations wrap these as UnknownOutcome.                                                                                                                                                                                            | Inspect safe typed cause and operation evidence.                                     |
| `Error::Closed`                        | Local Client or Interaction closed before observation/dispatch.                                                                                                                                                                                                                           | Rebind IDs in an open Client if more readback is needed.                             |

`RunOutcome.status` represents completed, waiting, failed or cancelled; returning an outcome does not make failed/cancelled successful. `http_response` retains original exact Run HTTP evidence; `Interaction.evidence` separately identifies submission, resume or recovered origin. Error formatting retains safe categories rather than raw URLs/credentials/untrusted bodies. Keep tokens and request payloads out of your own logs.

ObservationOptions freezes one deadline for queue, Run and stream; repeated result/iteration cannot extend it. An explicit absolute `deadline` also caps mutation time and includes elapsed submission time. Request-builder timeouts remain transport diagnostics, distinct from observation Timeout. Cancelling or dropping an operation ends local work only. `Interaction::close()` preserves only an already-cached result; `Run::interrupt()` is the explicit remote mutation.

Client clones and authored handles share one pool and authentication/cancellation state. Drop of one clone preserves retained handles. Explicit close on **any** clone cancels all shared local requests, including task-owned handles; the last owner releases transport. No background observer is left running when an Interaction drops.

Resume applies only to the exact idle last waiting Run and returns an Interaction for a **distinct** successor. Completed/failed/cancelled Runs continue through an explicit new normal message, not automatic retry/resume/fork. `ThreadView.last_run_id` is the last sealed Run of any outcome, not necessarily your submission. For preview and advanced committed display readback, see [streams and results](streams-and-results.md).
