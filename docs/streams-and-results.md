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

## Native AG-UI events and inline-child attribution

`Delta.event` is the native AG-UI 1.0 JSON map, not a text-only enum. Inspect `subagentRunId` before displaying `TEXT_MESSAGE_CONTENT` as root output: child and root message/tool IDs can collide. The outer Service `data.run_id` identifies this Interaction's Run, while an inner `runId` may describe an inline child. A child's `RUN_FINISHED` is only an observation; finite completion still comes from the exact authoritative Service Run.

Preserve `TOOL_CALL_RESULT.content` as either a string or an ordered list of structured text/media parts. Do not stringify a list, deduplicate repeated images, discard video/file sources or remove unknown CUSTOM values and nulls. `CUSTOM` authored user/steering input differs from generated system/tool content. Media descriptors can carry `payload_omitted` metadata instead of bytes; they are not downloadable media or a lossless transcript by themselves. Run Items keep structured content and `subagentRunId`; preserve those fields on readback instead of concatenating every Item into root text. The example prints full event maps and retained Items rather than implementing a UI reducer.

**When a stream stops early:** dropping the interaction or calling `close()` releases local observation; it does **not** interrupt the remote Run. After explicit close, `result()` works only if it was already cached. To deliberately stop remote work, use `outcome.run.interrupt().await?` (or bind its Run ID through `client.run(id)` and interrupt explicitly). If a read errors, keep the Thread/Entry IDs from the receipt and follow [recovery](errors-and-recovery.md); do not resend the POST blindly.

## Advanced Thread-wide events

`ThreadStream::open` is a separate **advanced protocol reader**, not the ordinary finite Agent interaction. It can yield unrelated Runs. If you hold an applied Run Items snapshot, supply its paired Run ID and display `position`. Its optional `resume_after` is only a Redis seek hint (`after`), not proof of display coverage. With coverage supplied, an absent, expired or incompatible hint falls back to retained replay filtered by position; hint absence alone does not imply a gap. Without a display baseline, omit both `run` and `position` for cursor-only observation; do not invent coverage from a Redis ID.

This standalone example reads an existing Run and prints its committed Items before observing its tail. Set `A13N_THREAD_ID` and `A13N_RUN_ID` as well as the URL/token. Printing is the example's application step, not durable business processing:

```rust
use a13n::{Client, Secret, streaming::{StreamOptions, ThreadFrame, ThreadStream}};
use std::{env, error::Error};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let client = Client::new(&env::var("A13N_SERVICE_URL")?, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let thread_id = env::var("A13N_THREAD_ID")?;
    let run = client.run(env::var("A13N_RUN_ID")?);
    let snapshot = run.items().get(Default::default()).await?.data;
    println!("Committed Items: {:#?}", snapshot.items);
    let position = snapshot.position.ok_or("No committed display yet; wait for a checkpoint")?;
    let mut reader = ThreadStream::open(client.resources().threads().at(thread_id), StreamOptions {
        run: Some(snapshot.run.id),
        position: Some(position),
        after: snapshot.resume_after.flatten(), // Omitted and explicit null both mean no hint.
        max_reconnects: 3,
        ..Default::default()
    }).await?;
    while let Some(frame) = reader.next().await? {
        match frame {
            ThreadFrame::Delta { data, .. } => println!("Provisional: {:?}", data.event),
            ThreadFrame::Boundary { data, .. } => println!("Readback now covers {}-{}", data.attempt, data.sequence),
            ThreadFrame::Gap(data) => {
                println!("Read Items for {} through {:?}; no automatic healing", data.run_id, data.position);
                break;
            }
            ThreadFrame::Reset(data) => {
                println!("Discard superseded provisional output and read Items for {}", data.run_id);
                break;
            }
            ThreadFrame::Changed(_) => println!("Re-read Thread metadata"),
        }
    }
    reader.close();
    Ok(())
}
```

A cursor-bearing frame is acknowledged only when the next `next()` poll starts **after your application applied it**. `applied_cursor()` is the prior applied Redis cursor; `applied_position()` advances only for contiguous deltas of the claimed Run and attempt. Other Runs do not advance that position. Gap/reset, attempt change or a missing sequence freezes coverage until you explicitly reopen from covering Run Items. Boundaries remain visible even at covered positions, but cannot bridge holes. Drop or `close()` never acknowledges the last received frame.

A gap's optional `position` identifies the output a snapshot must cover, not a new cursor to resume from. Missing/null means the target is unknown; a snapshot read alone does not prove recovery. Stop applying tail output across the hole, read Items, check coverage against the target, and explicitly open a new reader from the new applied snapshot. If the snapshot is still behind, wait for a newer boundary or terminal progress rather than repeatedly reading the same snapshot. Reset means a new attempt superseded provisional output; discard that suffix and read authoritative Items. The SDK supplies no Console reducer or automatic gap healing. Persist your own applied state and coverage together; receiving a frame is not a durable checkpoint. The generated `thread.stream().get(...)` also exposes raw SSE with the same query/header options for custom decoders.
