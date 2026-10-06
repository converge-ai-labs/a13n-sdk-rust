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
jq '{complete: .complete, baseline: .baseline, items: .items}' items.json
```

A `runs wait` exit of zero may still mean waiting, failed or cancelled; check `.data.status` before treating Items as a completed answer. `complete` means the Run is sealed, not all display history loaded. Default reads return the newest limit (default 200, max 500) plus the entire mutable tail, possibly exceeding the limit. Dense 1-based Item ordinals reveal earlier history when the first ordinal is greater than 1. Explicit `runs items get --run "$RUN" --before 20 --limit 10` or `--after 0` reads bounded historical windows; before >=1 and after >=0 exclude each other. Historical windows have `baseline=false` and null continuation/position/resume hint. They cannot seed a live consumer, heal gaps or seal its state; there is no `--all` cursor pager for Run Items. If the Entry was failed or withdrawn, stop and inspect it instead of reading an unrelated Run. Saving only transient text risks missing committed output. `runs interrupt "$RUN"` is a separate **explicit remote mutation**, unlike Ctrl-C or a local timeout.

## Resume from applied display coverage

For advanced snapshot-plus-tail consumers, read default Items (`baseline=true`) for the exact known Run, apply that display and recursive presentation continuation (not execution state), then open a **new** reader with its paired `run.id` and `position`. In the example above `RUN` is bound from the consumed Entry; to observe a still-running tail, you can read Items before `runs wait` once a checkpoint exists:

```bash
set -eu
a13n-service-cli runs result "$RUN" > baseline.json
jq -e '.baseline == true' baseline.json > /dev/null
jq '{items: .items, continuation: .continuation}' baseline.json # Apply display and recursive presentation state.
BASELINE_RUN=$(jq -er '.run.id' baseline.json)
POSITION=$(jq -er '.position' baseline.json) # Null: no checkpoint yet; wait before claiming coverage.
HINT=$(jq -r '.resume_after // empty' baseline.json)
if [ -n "$HINT" ]; then
  a13n-service-cli threads events --thread "$THREAD" --run "$BASELINE_RUN" \
    --position "$POSITION" --after "$HINT" --max-reconnects 3 > covered-events.jsonl
else
  a13n-service-cli threads events --thread "$THREAD" --run "$BASELINE_RUN" \
    --position "$POSITION" --max-reconnects 3 > covered-events.jsonl
fi
# Press Ctrl-C when done; a sealed Run does not make this Thread-wide command finite.
```

`--run` and `--position` must be supplied together. Position is canonical nonnegative `attempt-sequence`, not a Redis cursor. Service validates that the Run belongs to the authorized Thread and that the attempt is not newer than its latest attempt. Optional `resume_after`/`--after` is a confirmed Redis seek hint covered by the snapshot. A missing, trimmed or incompatible hint falls back to retained replay filtered by coverage; hint absence alone does not imply a gap. Omitting both coverage flags retains cursor-only semantics, including a gap for a removed cursor.

Delta/boundary JSONL includes the **prior** `applied_cursor` and `applied_position`: this line itself is only acknowledged after stdout flush, at the next read. Coverage advances only for contiguous deltas of the claimed Run and attempt. A gap, reset, attempt change or missing sequence freezes it until an explicit snapshot-based reopen; other Runs cannot advance it. Covered boundaries remain visible. A `gap` line has nullable `position` identifying the output readback must cover, not a resume hint. Do not apply later tail output across that hole or assume any snapshot read healed it. If readback is still behind the target, wait for a newer boundary or terminal progress before checking again. Unknown/null targets require reassessing continuity. Reset requires discarding superseded provisional output and reading Items for the new attempt. This command reports frames; it does not reconstruct or heal display state automatically.

For custom decoders, the generated raw command exposes the same queries and header, and saves SSE bytes instead of JSONL:

```bash
a13n-service-cli threads stream get --thread "$THREAD" --run "$BASELINE_RUN" \
  --position "$POSITION" --output raw-events.sse
# Add --last-event-id "$HINT" only when a hint exists; Ctrl-C remains local cancellation.
```
