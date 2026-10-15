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

`run.items().get(RunItemsGetOptions { limit: Some(2), ..Default::default() })` returns the newest limit plus the **entire mutable tail**, which can exceed the limit. The response contains only `run`, `items` and `complete`; it carries no stream progress. The default limit is 200 and maximum is 500. The SDK preserves the Service response without truncating the tail or fetching earlier history automatically.

For explicit history, use `a13n::resources::RunItemsGetOptions { before: Some(first_ordinal), limit: Some(200), ..Default::default() }` or `after: Some(last_ordinal)`. `before >= 1` and `after >= 0` are mutually exclusive. These bounded windows are committed readback even when `complete=true`; they do not establish stream coverage or heal a gap. Paging is by Item ordinal, not a collection cursor; there is no `pages()` or `--all` full-history loader for Run Items.

Generated `DisplayContinuation`, `FragmentState`, `ObserverContinuation` and `StreamPosition` preserve recursive presentation state, including child cursors and native JSON. They are not execution checkpoints or SDK-owned reducer state. They are carried by Session `items` frames; the SDK neither normalizes events nor implements a Console reducer.

## Native AG-UI events and inline-child attribution

`Delta.event` is the native AG-UI 1.0 JSON map, not a text-only enum. The sole delta wire form is raw `event` plus required nullable `item`: `item: null` is valid, omission is not. `ItemRef.ordinal`, `response_group` and arbitrary JSON `failure` preserve omission, explicit null and values. Inspect `subagentRunId` before displaying `TEXT_MESSAGE_CONTENT` as root output: child and root message/tool IDs can collide. The outer Service `data.run_id` identifies this Interaction's Run, while an inner `runId` may describe an inline child. A child's `RUN_FINISHED` is only an observation; finite completion still comes from the exact authoritative Service Run.

Preserve `TOOL_CALL_RESULT.content` as either a string or an ordered list of structured text/media parts. Do not stringify a list, deduplicate repeated images, discard video/file sources or remove unknown CUSTOM values and nulls. `CUSTOM` authored user/steering input differs from generated system/tool content. Media descriptors can carry `payload_omitted` metadata instead of bytes; they are not downloadable media or a lossless transcript by themselves. Run Items keep structured content and `subagentRunId`; preserve those fields on readback instead of concatenating every Item into root text. Raw access preserves full maps rather than implementing a UI reducer.

**When a stream stops early:** dropping the interaction or calling `close()` releases local observation; it does **not** interrupt the remote Run. After explicit close, `result()` works only if it was already cached. To deliberately stop remote work, use `outcome.run.interrupt().await?` (or bind its Run ID through `client.run(id)` and interrupt explicitly). If a read errors, keep the Thread/Entry IDs from the Interaction identity and follow [recovery](errors-and-recovery.md); do not resend the POST blindly.

## Advanced native Session events

`SessionStream::open(session_resource, SessionStreamOptions)` observes one connection to the native Session stream. Every connection begins with a fresh `thread` snapshot for each Thread and an `items` snapshot only for its current Run, followed by native `delta`, replacement `items`, metadata `thread` and `gap` frames. A Thread with no current Run can produce only its `thread` frame until later changes; its last sealed Run's Items require explicit Run readback. Session frames carry no SSE IDs, Last-Event-ID or cursor options. Unknown event names are ignored; malformed known frames fail the reader. `response()` retains HTTP status/headers, and `close()` releases only local observation. Session snapshots have no protocol size bound, so the default adds no frame-size limit; applications may set `SessionStreamOptions.max_frame_bytes = Some(bytes)` to enforce their own budget.

This complete example reads the first Thread snapshot of an existing Session within ten seconds, then closes local observation. It works when that Thread has an active Run or only a last sealed Run. An empty Session reaches the timeout. Supply `A13N_SESSION_ID` as well as URL/token:

```rust
use a13n::{Client, Secret, streaming::{SessionFrame, SessionStream}};
use std::{env, error::Error, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let client = Client::new(&env::var("A13N_SERVICE_URL")?, Secret::new(env::var("A13N_API_TOKEN")?))?;
    let session_id = env::var("A13N_SESSION_ID")?;
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut reader = SessionStream::open(client.api().sessions().at(session_id), Default::default()).await?;
        if let Some(SessionFrame::Thread(state)) = reader.next().await? {
            println!("Thread: {:?}", state);
        }
        reader.close();
        Ok::<_, Box<dyn Error>>(())
    }).await??;
    Ok(())
}
```

A Session `items` frame replaces the receiver's suffix from `first` onward; earlier Items are final. Its required nullable `continuation` carries presentation state. `complete` seals only that Run's display, not the Session connection. A `gap` requires authoritative readback and reassessing continuity. The SDK returns typed frames without maintaining display state or assembling fragments. After EOF/error, explicitly open a new reader for a fresh snapshot; that snapshot can omit already sealed Runs, whose historical Items remain available through Run readback.

## Advanced Thread-wide events

`ThreadStream::open` remains a separate reader for `delta`, `boundary`, `changed`, `reset` and `gap`. It may yield unrelated Runs. Applications that already hold applied progress may provide paired `StreamOptions.run` and `position`, plus optional `after` as a Redis seek hint. Run Items no longer supplies any of those coordinates. Omit both progress fields for cursor-only observation; never derive a position from an Item ordinal or Redis ID.

A cursor-bearing frame is acknowledged when the next `next()` poll starts after application processing. `applied_cursor()` and `applied_position()` describe prior applied progress; close/drop do not acknowledge the last received frame. Coverage advances only through contiguous deltas of the claimed Run/attempt. Gaps, resets, attempt changes or missing sequences freeze it; other Runs and boundaries cannot bridge holes. Reopen only with application-established coverage, or choose a fresh Session snapshot. A gap's optional `position` is a recovery target, not a resume cursor. The raw generated `thread.stream().get(...)` retains the same explicit query/header controls.

Saved Items may carry `content_refs` for large fields: each reference preserves its preview, original byte size, media type and truncation. Read one explicitly with `client.api().runs().at(run_id).contents().at(content_id).get(Default::default()).await?`; `RunContent.value` is native JSON. A truncated JSON value can be a text prefix rather than a parseable JSON object. Neither Items reads nor `next_text` automatically expand references, and final Run output remains separate. Live `ItemRef.memory_sources` and saved item content preserve automatic Memory delivery evidence; these sources do not establish model citations or enumerate every explicit Memory tool read.
