# Start, continue and wait for an Agent

Use an existing Agent **ID** and workspace API key; configure the CLI as in [connection and profiles](connection-and-profiles.md). Examples use `jq` to inspect JSON. A Thread groups messages but has no permanent Agent: `agents send` names the Agent again.

## Start and continue with a result

Give each *different* submission a different caller-owned idempotency key. Save the returned Thread ID; do not derive it from a Run ID:

```bash
AGENT_ID='your-existing-agent-id'
a13n-service-cli --include-meta agents start "$AGENT_ID" \
  --text 'Summarize this project' --idempotency-key 'unique-start-key' \
  --wait > interaction.json
jq '{thread: .submitted.data.thread.id, run: .outcome.data.id, status: .outcome.data.status}' interaction.json
THREAD=$(jq -er '.submitted.data.thread.id' interaction.json)
STATUS=$(jq -er '.outcome.data.status' interaction.json)
if [ "$STATUS" = completed ]; then
  a13n-service-cli agents send "$AGENT_ID" --thread "$THREAD" \
    --text 'Explain the trade-off' --idempotency-key 'unique-followup-key' \
    --wait > follow-up.json
  cat follow-up.json
fi
```

`--wait` returns acceptance **and** the exact incorporating Run outcome, not just the receipt. A command may exit zero with a `waiting`, `failed` or `cancelled` Run; check `.outcome.data.status`. A failed/withdrawn queued Entry instead produces an error. For a completed Run, `a13n-service-cli runs result RUN_ID` reads its committed Items. To save receipt IDs immediately, omit `--wait` and observe the Entry below.

## Import completed model context on creation

The ordinary `agents start` helper sends text only. Use generated `threads create` with `--body` to import completed Pydantic AI model messages **and** submit a new first payload in the same request. Replace the example Agent ID and request key; this changes Service state:

```bash
cat > imported-thread.json <<'JSON'
{"agent_id":"your-existing-agent-id","message_history":[{"kind":"request","parts":[{"part_kind":"user-prompt","content":"Earlier question"}]},{"kind":"response","parts":[{"part_kind":"text","content":"Earlier answer"}]}],"payload":{"content":[{"type":"text","text":"Continue this conversation"}]}}
JSON
a13n-service-cli --include-meta threads create --body @imported-thread.json \
  --idempotency-key 'unique-import-key' > imported.json
THREAD=$(jq -er '.data.thread.id' imported.json)
a13n-service-cli --include-meta threads get "$THREAD" > thread.json
jq '.data.message_history' thread.json
```

The readback is the submitted native JSON, not historical Runs or UI Items. The Service validates completed user text, model text and closed tool-call/result exchanges (at most 256 messages and 262144 UTF-8 bytes); instructions, media and unresolved calls are rejected. Use `threads inbox create` or `agents send` for a later message **without** `message_history`. Imports are immutable; forks inherit their checkpoint instead of accepting a second import. `threads create --schema` exposes the `message_history` property, while a local dry-run cannot prove the Service will accept its content.

## A message queued behind another Run

An accepted response may contain `"run": null`. Assignment alone is not consumption: a queued Entry can return to pending. This script submits another message to a known Thread, then observes only **its** Entry and exact Run:

```bash
a13n-service-cli --include-meta agents send "$AGENT_ID" --thread "$THREAD" \
  --text 'Follow up after the current Run' --idempotency-key 'unique-queued-key' \
  > submitted.json
ENTRY=$(jq -er '.data.entry.id' submitted.json)
a13n-service-cli --include-meta threads inbox wait "$ENTRY" --thread "$THREAD" \
  --timeout 90 > entry.json
ENTRY_STATUS=$(jq -er '.data.status' entry.json)
if [ "$ENTRY_STATUS" = consumed ]; then
  RUN=$(jq -er '.data.assigned_run_id' entry.json)
  a13n-service-cli --include-meta runs wait "$RUN" --timeout 90 > run.json
  jq '.data.status' run.json
else
  printf 'Entry ended as %s; inspect entry.json\n' "$ENTRY_STATUS" >&2
fi
```

`threads inbox wait` stops at consumed, failed or withdrawn. If the Thread's head is waiting, an ordinary message remains pending until that head is explicitly resumed. A local timeout or Ctrl-C ends observation, not durable Service work. Each invocation has its own timeout; a multi-command script needs its own overall deadline. Never use a Thread's latest Run as a shortcut.

## Continue the exact waiting Run

A waiting Run has `pending.approvals` and `pending.calls`. Every pending ID must have a matching result in its **own** category, even when the batch contains only one call. `calls` includes client tools and built-in questions. These independent shell scripts use `set -eu` and stop if the observed pending set differs; choose only the branch that matches a real waiting Run.

For a tool named `lookup_inventory` with a string `sku` argument, put actual data in `inventory.json` (for example, `printf '{"A-100":3}\n' > inventory.json`). Substitute your real application operation before claiming its result. This example submits a returned call result and ordinary text in one atomic resume:

```bash
set -eu
jq -e '.outcome.data.status == "waiting" and
       (.outcome.data.pending.approvals | length == 0) and
       (.outcome.data.pending.calls | length == 1) and
       .outcome.data.pending.calls[0].tool_name == "lookup_inventory" and
       (.outcome.data.pending.calls[0].arguments.sku | type) == "string"' \
  interaction.json > /dev/null
RUN=$(jq -er '.outcome.data.id' interaction.json)
CALL_ID=$(jq -er '.outcome.data.pending.calls[0].tool_call_id' interaction.json)
SKU=$(jq -er '.outcome.data.pending.calls[0].arguments.sku' interaction.json)
QUANTITY=$(jq -er --arg sku "$SKU" \
  '.[$sku] | select(type == "number" and . >= 0 and floor == .)' inventory.json)
jq -n --arg sku "$SKU" --argjson quantity "$QUANTITY" \
  '{sku: $sku, quantity: $quantity}' > tool-result.json
jq -n --arg id "$CALL_ID" --slurpfile result tool-result.json \
  '{approvals: {}, calls: {($id): {status: "returned", value: $result[0]}},
    input: {content: [{type: "text", text: "Also review this context."}]}}' > resume.json
a13n-service-cli --include-meta runs resume "$RUN" --body @resume.json \
  --idempotency-key 'unique-client-tool-resume-key' > resumed.json
SUCCESSOR=$(jq -er '.data.id' resumed.json)
a13n-service-cli --include-meta runs wait "$SUCCESSOR" --timeout 90 > successor.json
jq '.data.status' successor.json
```

Unknown tools, invalid arguments and missing SKUs stop without a POST. `input.content` can also reference an already-published Asset with `{ "type": "asset", "asset_id": "..." }`, subject to Service access and the successor's frozen configuration; it cannot replace `calls`. To **deny or approve** a server-side action, inspect the presented request and obtain an authorized reviewer's explicit decision:

```bash
set -eu
jq -e '.outcome.data.status == "waiting" and
       (.outcome.data.pending.approvals | length == 1) and
       (.outcome.data.pending.calls | length == 0)' interaction.json > /dev/null
jq '.outcome.data.pending.approvals[0] | {tool_name, arguments, presentation}' interaction.json
RUN=$(jq -er '.outcome.data.id' interaction.json)
CALL_ID=$(jq -er '.outcome.data.pending.approvals[0].tool_call_id' interaction.json)
printf 'Reviewer decision (approve or deny): ' >&2
IFS= read -r decision
case "$decision" in
  approve)
    jq -n --arg id "$CALL_ID" \
      '{approvals: {($id): {action: "approve"}}, calls: {}}' > decision.json ;;
  deny)
    printf 'Reason for denial: ' >&2
    IFS= read -r reason
    test -n "$reason"
    jq -n --arg id "$CALL_ID" --arg reason "$reason" \
      '{approvals: {($id): {action: "deny", reason: $reason}}, calls: {}}' > decision.json ;;
  *) printf 'No explicit decision; Run remains waiting\n' >&2; exit 2 ;;
esac
a13n-service-cli --include-meta runs resume "$RUN" --body @decision.json \
  --idempotency-key 'unique-approval-resume-key' > resumed.json
SUCCESSOR=$(jq -er '.data.id' resumed.json)
a13n-service-cli --include-meta runs wait "$SUCCESSOR" --timeout 90 > successor.json
jq '.data.status' successor.json
```

A **built-in question** also appears under `pending.calls`, normally with tool name `ask_user_question`. Read its actual arguments, ask the person, and write `question-answer.json` in the Harness `UserQuestionAnswers` shape, for example `{"answers":{"actual question text":"selected choice"}}`. Do not assume the example choice is available: the Service checks the value against the exact pending call. Resume with a call result for that question ID, not `agents send`:

```bash
set -eu
jq -e '.outcome.data.status == "waiting" and
       (.outcome.data.pending.approvals | length == 0) and
       (.outcome.data.pending.calls | length == 1) and
       .outcome.data.pending.calls[0].tool_name == "ask_user_question"' \
  interaction.json > /dev/null
jq '.outcome.data.pending.calls[0] | {tool_name, arguments}' interaction.json
RUN=$(jq -er '.outcome.data.id' interaction.json)
CALL_ID=$(jq -er '.outcome.data.pending.calls[0].tool_call_id' interaction.json)
jq -e 'type == "object" and (.answers | type == "object")' question-answer.json > /dev/null
jq -n --arg id "$CALL_ID" --slurpfile answer question-answer.json \
  '{approvals: {}, calls: {($id): {status: "returned", value: $answer[0]}}}' > question-resume.json
a13n-service-cli --include-meta runs resume "$RUN" --body @question-resume.json \
  --idempotency-key 'unique-question-resume-key' > resumed.json
SUCCESSOR=$(jq -er '.data.id' resumed.json)
a13n-service-cli --include-meta runs wait "$SUCCESSOR" --timeout 90 > successor.json
jq '.data.status' successor.json
```

For an intentional unanswered question or a failed client tool, use `{ "status": "failed", "message": "No response was given" }` under its call ID. The Service requires **exact complete coverage** across the approval and call maps: missing, extra or miscategorized IDs reject the entire request; there are no omission defaults and no partial batches. Optional `input` adds user content to the **same immutable intent**, after the deferred results in model context. It cannot answer a question, authorize an approval or fill a missing tool result. The successor has its own Run ID; `runs wait "$RUN"` still observes the original waiting Run. Check the successor status and [committed Items](events-and-readback.md) before claiming completion. Preserve the original key and receipt after an uncertain POST; see [scripting and errors](scripting-and-errors.md).
