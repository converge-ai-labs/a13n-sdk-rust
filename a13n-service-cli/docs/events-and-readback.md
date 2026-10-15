# Observe events, then read committed Items

The CLI's `threads events` command follows a **Thread-wide** stream; it may contain multiple Runs. Use it when you need live JSONL progress, not as a replacement for a finite Agent result. The Rust SDK offers a finite per-submission `Interaction.next()`; this CLI command deliberately exposes the broader stream.

After [connecting](connection-and-profiles.md), submit work without `--wait` so you can attach while it runs. Use an existing Agent ID and a fresh request key:

```bash
AGENT_ID='your-existing-agent-id'
a13n-service-cli --include-meta agents start "$AGENT_ID" \
  --text 'Describe the proposed change' --idempotency-key 'unique-stream-request' \
  > submitted.json
THREAD=$(jq -er '.data.thread.id' submitted.json)
a13n-service-cli threads events --thread "$THREAD" --max-reconnects 3 > events.jsonl
# In this foreground example, press Ctrl-C when you have observed enough frames.
```

The command writes one JSON object per line and flushes before advancing. A cursor-bearing line has a `cursor`; some lines are `changed`, `gap` or `reset` readback hints rather than text. For example, `jq -c 'select(.type == "delta")' events.jsonl` shows received deltas after you stop the observer. Ctrl-C exits 130 and closes only local observation; the remote Run keeps executing. A `--timeout 30` would similarly end this local command (exit 5), not interrupt remote work.

## Native media and child attribution

Each delta has raw `event` plus required nullable `item`, never a second typed-operation format. Item references preserve omitted/null/value `ordinal`, `response_group` and arbitrary JSON `failure`. Each delta's `event` is the unchanged native AG-UI 1.0 JSON map, including unknown CUSTOM values, nulls, media metadata and ordered structured `TOOL_CALL_RESULT.content`. Text-only filtering must inspect `event.subagentRunId`: inline-child message/tool IDs can collide with root IDs. Do not flatten child text into a root reply, stringify structured parts, deduplicate repeated media or interpret omitted-payload descriptors as original bytes. The outer `run_id` is the Service owner; inner child `RUN_FINISHED` is not authoritative root completion. Use exact `runs wait` and committed Items, whose content retains child attribution. Authored user/steering input CUSTOM events are display evidence, not permission to execute tools. The command prints frames without a UI reducer.

## Recover with a cursor you actually applied

If your consumer has **durably applied** an event and saved its cursor together with its own state, reconnect with that cursor:

```bash
APPLIED_CURSOR='your-durably-applied-cursor'
a13n-service-cli threads events --thread "$THREAD" --after "$APPLIED_CURSOR" \
  --max-reconnects 3 > resumed-events.jsonl
# Press Ctrl-C when done.
```

The CLI considers writing to stdout an applied frame, but writing a line into `events.jsonl` does not prove that your downstream consumer committed its business change. Do not just take the last received cursor and skip processing on restart. Reconnection is bounded; retained frames can be missing, and the stream is not a full history. A `gap` or `reset` for a Run requires authoritative Items readback; `changed` calls for Thread readback.

To inspect the exact Run incorporated by **your** submission, wait for its Entry to be consumed as shown in [Agent workflows](agent-workflows.md#a-message-queued-behind-another-run). An accepted receipt's Run can still be a provisional assignment that rolls back; it is not enough for exact readback:

```bash
set -eu
ENTRY=$(jq -er '.data.entry.id' submitted.json)
a13n-service-cli --include-meta threads inbox wait "$ENTRY" --thread "$THREAD" \
  --timeout 90 > entry.json
jq -e '.data.status == "consumed"' entry.json > /dev/null
RUN=$(jq -er '.data.assigned_run_id' entry.json)
a13n-service-cli --include-meta runs wait "$RUN" --timeout 90 > run.json
jq '.data.status' run.json
a13n-service-cli runs result "$RUN" > items.json
jq '{complete: .complete, items: .items}' items.json
```

A `runs wait` exit of zero may still mean waiting, failed or cancelled; check `.data.status` before treating Items as a completed answer. `complete` means the Run is sealed, not all display history loaded. Default reads return the newest limit (default 200, max 500) plus the entire mutable tail, possibly exceeding the limit. Dense 1-based Item ordinals reveal earlier history when the first ordinal is greater than 1. Explicit `runs items get --run "$RUN" --before 20 --limit 10` or `--after 0` reads bounded historical windows; before >=1 and after >=0 exclude each other. Items responses contain only `run`, `items` and `complete`, without stream positions or continuation. They cannot establish stream coverage or heal gaps; there is no `--all` cursor pager for Run Items. If the Entry was failed or withdrawn, stop and inspect it instead of reading an unrelated Run. Saving only transient text risks missing committed output. `runs interrupt "$RUN"` is a separate **explicit remote mutation**, unlike Ctrl-C or a local timeout.

## Native Session snapshots and events

Use the Session ID from your own submitted Thread for snapshot-based observation:

```bash
SESSION=$(jq -er '.data.thread.session_id' submitted.json)
a13n-service-cli sessions events "$SESSION" --timeout 30 > session-events.jsonl
# Timeout exits 5 and closes local observation; Ctrl-C exits 130.
```

The command emits typed `thread`, `items`, `delta` and `gap` JSONL and flushes every line. A `thread` frame carries current metadata, nullable current/last Runs and pending Entries. An `items` frame replaces the suffix from `first` onward and carries required nullable display `continuation`; `complete` seals that Run's display, not the Session. Native `delta.event` and required nullable `delta.item` retain native JSON. A `gap` calls for committed Run readback and reassessing continuity. The CLI does not reconstruct display state.

Every connection begins with fresh snapshots. Session frames have no SSE ID, Last-Event-ID or resume flags. EOF/error ends this reader; run the command again explicitly to obtain fresh snapshots. Already sealed Runs may be absent from a new snapshot; use `runs items get` for their history. Unknown event names are ignored and malformed known frames fail.

For a custom decoder, save native bytes:

```bash
a13n-service-cli sessions stream get --session "$SESSION" --output session.sse --timeout 30
```

Thread events remain available separately. If your application has established applied display progress itself, `threads events --thread "$THREAD" --run "$RUN" --position "$APPLIED_POSITION"` supplies the paired claim; optional `--after "$APPLIED_CURSOR"` is a Redis seek hint. Run Items no longer yields progress. Omit both claim flags for cursor-only observation. Delta/boundary lines report prior `applied_cursor`/`applied_position` and acknowledge this line only after stdout flush at the next poll. Gaps/resets/missing sequences freeze coverage; no automatic readback heals it. The raw `threads stream get` command retains explicit `--run`, `--position` and `--last-event-id` controls.

## Read execution configuration, Attempts, metrics and screenshots

Use exact IDs from your incorporated submission:

```bash
a13n-service-cli runs list --thread-id "$THREAD"
a13n-service-cli runs agent get --run "$RUN"
a13n-service-cli runs attempts list --run "$RUN"
# ATTEMPT and RECORD are known IDs from your application or tracing/usage integration.
a13n-service-cli runs attempts get "$ATTEMPT" --run "$RUN"
a13n-service-cli usage get --run-id "$RUN"
a13n-service-cli usage records metrics get --record "$RECORD"
```

Executed Agent readback contains its frozen revision and config with frozen overrides, while referenced models/connections/templates stay live. It is distinct from current Agent defaults and Run configuration. Usage timing fields are required nullable values: null and zero remain distinct. Output tokens/sec includes full request time, including first-chunk wait; Service owns the calculation. The API has no usage record-list operation.

An Item's `content.tool_images` contains retained screenshot metadata. Fetch a selected reference explicitly with `runs images get "$IMAGE" --run "$RUN" --output screenshot.png`. It streams raw PNG/JPEG bytes, separate from JSON/text `runs contents get`; no event/Item read automatically fetches images. Readiness checks do not grant input permissions, and a retained image ID is not a fresh Computer observation handle. Inspect/reconcile an input unknown outcome rather than replaying it to repair a failed display fetch.
