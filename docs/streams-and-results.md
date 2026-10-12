# Stream progress and read the final result

Use `Interaction.next_text()` for optional live root-assistant text preview. Result-only callers skip iteration and call `result()` directly. Both share one finite observer and one fixed deadline; no second subscription or background transcript is created.

Supply Service URL/token/Agent ID and use the quick-start dependencies. This complete example deliberately continues to authoritative result if conservative preview stops:

```rust
use a13n::{Client, Error, Secret};
use std::{env, error::Error as StdError};

#[tokio::main]
async fn main() -> Result<(), Box<dyn StdError>> {
    let client = Client::new(&env::var("A13N_SERVICE_URL")?, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let mut interaction = client.agent(env::var("A13N_AGENT_ID")?).start("Explain the migration").await?;
    println!("Identity: {:?}", interaction.identity);
    loop {
        match interaction.next_text().await {
            Ok(Some(text)) => print!("{text}"),
            Ok(None) => break,
            Err(Error::PreviewInterrupted(error)) => { eprintln!("{error}"); break; }
            Err(other) => return Err(other.into()),
        }
    }
    let outcome = interaction.result().await?;
    println!("\nAuthoritative {:?}: {}", outcome.status, outcome.output);
    interaction.close(); // Local cleanup only; the result is already cached.
    Ok(())
}
```

Preview emits only string CONTENT matched to a trustworthy root assistant START. It excludes `subagentRunId` children and explicit `metadata.display:false`, retaining at most **eight active root message identities** and no completed history or text. Interleaving within that bound is supported; exhaustion interrupts rather than silently evicting. Empty string chunks stay empty. Native message IDs can collide across children; a child RUN_FINISHED never seals the root.

Gap/reset, missing/ambiguous START, relevant malformed protocol and trustworthy **outer Service per-attempt** sequence discontinuity interrupt preview. Inner native/custom sequence fields and foreign Run coordinates do not define holes. Unknown unframed CUSTOM events are ignored by preview but preserved by raw iteration. An unexcluded root `CUSTOM` named `a13n.stream.fragment` conservatively interrupts: its opaque `value:{id,index,count,data}` frames ORIGINAL CUSTOM JSON, not a text-message suffix. A fragment has no reliable inner relevance/name field. This helper does not assemble it, inspect opaque IDs or infer semantics from partial data. Unsupported framing does **not** mean every fragment contains assistant text. Child/hidden fragments are excluded first.

Preview interruption is typed and sticky: another `next_text()` reports the same error without reopening the stream. The Interaction remains open for `result()` and deliberate sequential raw `next()` calls. `&mut` borrowing prevents simultaneous readers; raw/text use consumes the same reader, not independent replay. No baseline repair, full display reducer or fragment assembly happens here. Preview is transient, not a lossless transcript. Full final Run output remains authoritative even if no preview was emitted or preview stopped.

Raw `interaction.next().await?` yields exact-Run `ThreadFrame` values with complete event maps, boundaries and gap/reset hints. The finite loop seals from exact Run status even with idle/busy SSE or blocked headers. A queued Entry binds only after consumed, not assigned. `outcome.http_response` explicitly retains exact Run HTTP metadata. The optional text field is populated only from string output; structured output is not extracted from Items or concatenated events.

Breaking iteration leaves the Interaction open. Call `result()` to finish local observation or `close()`/drop to stop it. Explicit close permits only an already-cached result; it never interrupts remote execution. To stop remote work, explicitly call `Run::interrupt()`. Save IDs/key for [recovery](errors-and-recovery.md); never resend a POST just because a stream stopped.

## Advanced committed display readback

Run Items are authoritative for the returned **committed display window**, not the final-reply accessor or necessarily a full lifetime transcript. `complete` means sealed, not full history loaded; dense 1-based ordinals identify earlier history. Gap/reset readback and UI reducers remain application responsibilities.

## Read recent or historical ordinal windows

`run.items().get(RunItemsGetOptions { limit: Some(2), ..Default::default() })` returns the newest limit plus the **entire mutable tail**, which can exceed the limit. With no `before` or `after`, `baseline=true`; its nullable `position`, optional nullable `continuation` and `resume_after` describe committed display coverage. The default limit is 200 and maximum is 500. The SDK preserves the Service response without truncating the tail or fetching earlier history automatically.

For explicit history, use `a13n::resources::RunItemsGetOptions { before: Some(first_ordinal), limit: Some(200), ..Default::default() }` or `after: Some(last_ordinal)`. `before >= 1` and `after >= 0` are mutually exclusive. These bounded windows have `baseline=false` and null continuation/position/resume hint even when `complete=true`. Do not install them as live baselines, advance stream coverage, heal a gap or seal a live consumer with them. Paging is by Item ordinal, not a collection cursor; there is no `pages()` or `--all` full-history loader for Run Items.

Generated `DisplayContinuation`, `FragmentState`, `ObserverContinuation` and `StreamPosition` preserve recursive presentation state, including child cursors and native JSON. They are not execution checkpoints or SDK-owned reducer state. Preserve them when your application applies a baseline; the SDK neither normalizes events nor implements a Console reducer.

## Native AG-UI events and inline-child attribution

`Delta.event` is the native AG-UI 1.0 JSON map, not a text-only enum. The sole delta wire form is raw `event` plus required nullable `item`: `item: null` is valid, omission is not. `ItemRef.ordinal`, `response_group` and arbitrary JSON `failure` preserve omission, explicit null and values. Inspect `subagentRunId` before displaying `TEXT_MESSAGE_CONTENT` as root output: child and root message/tool IDs can collide. The outer Service `data.run_id` identifies this Interaction's Run, while an inner `runId` may describe an inline child. A child's `RUN_FINISHED` is only an observation; finite completion still comes from the exact authoritative Service Run.

Preserve `TOOL_CALL_RESULT.content` as either a string or an ordered list of structured text/media parts. Do not stringify a list, deduplicate repeated images, discard video/file sources or remove unknown CUSTOM values and nulls. `CUSTOM` authored user/steering input differs from generated system/tool content. Media descriptors can carry `payload_omitted` metadata instead of bytes; they are not downloadable media or a lossless transcript by themselves. Run Items keep structured content and `subagentRunId`; preserve those fields on readback instead of concatenating every Item into root text. Raw access preserves full maps rather than implementing a UI reducer.

**When a stream stops early:** dropping the interaction or calling `close()` releases local observation; it does **not** interrupt the remote Run. After explicit close, `result()` works only if it was already cached. To deliberately stop remote work, use `outcome.run.interrupt().await?` (or bind its Run ID through `client.run(id)` and interrupt explicitly). If a read errors, keep the Thread/Entry IDs from the Interaction identity and follow [recovery](errors-and-recovery.md); do not resend the POST blindly.

## Advanced Thread-wide events

`ThreadStream::open` is a separate **advanced protocol reader**, not the ordinary finite Agent interaction. It can yield unrelated Runs. If you hold an applied Run Items snapshot with `baseline=true` and a committed position, supply its paired Run ID and display `position`. Historical windows cannot establish this coverage. Its optional `resume_after` is only a Redis seek hint (`after`), not proof of display coverage. With coverage supplied, an absent, expired or incompatible hint falls back to retained replay filtered by position; hint absence alone does not imply a gap. Without a display baseline, omit both `run` and `position` for cursor-only observation; do not invent coverage from a Redis ID.

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
    if !snapshot.baseline { return Err("Historical window cannot seed a live reader".into()); }
    println!("Committed Items: {:#?}", snapshot.items);
    let position = snapshot.position.ok_or("No committed display yet; wait for a checkpoint")?;
    let mut reader = ThreadStream::open(client.api().threads().at(thread_id), StreamOptions {
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

A gap's optional `position` identifies the output a snapshot must cover, not a new cursor to resume from. Missing/null means the target is unknown; a snapshot read alone does not prove recovery. Stop applying tail output across the hole, read default Items with `baseline=true`, apply its continuation, check coverage against the target, and explicitly open a new reader from the new applied snapshot. If the snapshot is still behind, wait for a newer boundary or terminal progress rather than repeatedly reading the same snapshot. Reset means a new attempt superseded provisional output; discard that suffix and read authoritative Items. The SDK supplies no Console reducer or automatic gap healing. Persist your own applied state and coverage together; receiving a frame is not a durable checkpoint. The generated `thread.stream().get(...)` also exposes raw SSE with the same query/header options for custom decoders.
