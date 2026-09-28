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
jq '{complete: .complete, dropped: .dropped, items: .items}' items.json
```

A `runs wait` exit of zero may still mean waiting, failed or cancelled; check `.data.status` before treating Items as a completed answer. `complete` says whether the display is sealed, and `dropped` counts earliest Items lost to its display limit; even committed Items need not be a full historical transcript. If the Entry was failed or withdrawn, stop and inspect it instead of reading an unrelated Run. Saving only transient text risks missing committed output. `runs interrupt "$RUN"` is a separate **explicit remote mutation**, unlike Ctrl-C or a local timeout.
