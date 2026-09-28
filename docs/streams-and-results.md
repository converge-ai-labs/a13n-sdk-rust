# Stream progress and read the final result

Use `Interaction.next()` when you want live progress. It observes only the Run that incorporated this submission. If you only need the result, skip the stream and call `interaction.result().await?` as in the [quick start](../README.md#get-started-from-source).

Provide the Service URL, workspace key, an existing Agent ID, and a fresh `A13N_REQUEST_KEY`. The sibling application's dependencies from the quick start (`a13n` and `tokio`) are sufficient.

```rust
use a13n::{Client, Secret, generated::models, streaming::ThreadFrame};
use std::{env, error::Error};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let url = env::var("A13N_SERVICE_URL")?;
    let client = Client::new(&url, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let agent = client.agent(env::var("A13N_AGENT_ID")?);
    let mut interaction = agent.start("Explain the migration", env::var("A13N_REQUEST_KEY")?).await?;
    println!("Thread: {}", interaction.thread.id);

    while let Some(frame) = interaction.next().await? {
        match frame {
            ThreadFrame::Delta { data, .. } => println!("Transient delta: {:?}", data.event),
            ThreadFrame::Boundary { data, .. } => println!("Checkpoint for Run {}", data.run_id),
            ThreadFrame::Gap(_) | ThreadFrame::Reset(_) => println!("Read committed Run items after completion"),
            ThreadFrame::Changed(_) => {} // No Thread-wide changes are emitted by this interaction.
        }
    }
    let outcome = interaction.result().await?;
    println!("Final status: {:?}", outcome.status());
    if *outcome.status() == models::RunStatus::Completed {
        let committed = outcome.run.items().get(Default::default()).await?;
        println!("Sealed: {}, earliest items dropped: {}", committed.data.complete, committed.data.dropped);
        println!("Retained committed items: {:#?}", committed.data.items);
    }
    Ok(())
}
```

The loop ends naturally at completed, waiting, failed or cancelled, including when the SSE socket is idle or execution completes before the connection opens. A queued submission waits for its Entry to be **consumed** before yielding attributed deltas. `outcome.snapshot` retains the exact Run response, including HTTP status and headers; `outcome.output()` is optional Run output and need not be conversation text. Run Items are authoritative for the committed display, but not necessarily a complete lifetime transcript: check `complete` for a sealed Run and `dropped` for items removed by the display limit. Retained frame replay is not lossless history, and gaps or resets call for readback even if transient output looked complete. If `dropped` is nonzero, do not promise the reader that earlier Items can be reconstructed from SSE.

**When a stream stops early:** dropping the interaction or calling `close()` releases local observation; it does **not** interrupt the remote Run. After explicit close, `result()` works only if it was already cached. To deliberately stop remote work, use `outcome.run.interrupt().await?` (or bind its Run ID through `client.run(id)` and interrupt explicitly). If a read errors, keep the Thread/Entry IDs from the receipt and follow [recovery](errors-and-recovery.md); do not resend the POST blindly.

## Advanced Thread-wide events

If you must observe more than one Run in a Thread, `ThreadStream::open(client.resources().threads().at(thread_id), StreamOptions { after, max_reconnects, ..Default::default() }).await?` is a separate **advanced protocol reader**, not the ordinary finite Agent interaction. It can yield unrelated Runs. A cursor-bearing frame is applied only when the next `next()` poll starts; persist your application state and the last applied cursor together. A received cursor alone is not a durable checkpoint. It supports bounded reconnection; `Changed`, `Gap` and `Reset` ask your application to read Thread or Run Items instead of manufacturing missed text. The generated `thread.stream().get(...)` also exposes raw SSE for custom decoders.
