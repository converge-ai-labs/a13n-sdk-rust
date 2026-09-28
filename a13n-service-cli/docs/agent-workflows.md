# Start, continue and wait for an Agent

Use an existing Agent **ID** and workspace API key; configure the CLI as in [connection and profiles](connection-and-profiles.md). These examples also use `jq` to inspect JSON. A Thread groups messages but has no permanent Agent: `agents send` names the Agent again.

## Start and continue with a result

Give each *different* submission a different caller-owned idempotency key. Save the returned Thread ID; do not try to derive it from a Run ID:

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

`--wait` returns acceptance **and** the exact incorporating Run outcome, not just the receipt. A command may exit zero with a `waiting`, `failed` or `cancelled` Run; check `.outcome.data.status`. A failed/withdrawn queued Entry instead produces an error. For a completed Run, `a13n-service-cli runs result RUN_ID` reads its committed Items. If you need receipt IDs immediately even when waiting may take a long time, omit `--wait`, save the acceptance response, and observe the Entry explicitly below.

## A message queued behind another Run

The accepted response may contain `"run": null`. Assignment alone is not consumption: a queued Entry can return to pending. This script submits another message to a known Thread, then observes only **its** Entry and exact Run:

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

`threads inbox wait` stops at consumed, failed or withdrawn. A local timeout or Ctrl-C ends observation, not the queued Service work. Each invocation has its own timeout; a multi-command script needs its own overall deadline. Do not grab a Thread's latest Run—it may belong to another submission.

## Continue a waiting Run by its pending kind

Choose **one** of these branches after inspecting the complete `.outcome.data.pending.items` array in a `waiting` response. Each script checks that the array has exactly one item and stops on failed `jq` validation; do not pick `[0]` from a multi-action batch and silently answer only part of it. Use an actual client tool named `lookup_inventory` with a string `sku` argument for this example. Put your authoritative inventory data in `inventory.json`, for example `printf '{"A-100":3}\n' > inventory.json`, then replace the file lookup with your application's real tool operation:

```bash
set -eu
jq -e '.outcome.data.status == "waiting" and
       (.outcome.data.pending.items | length == 1) and
       .outcome.data.pending.items[0].kind == "client_tool" and
       .outcome.data.pending.items[0].tool_name == "lookup_inventory" and
       (.outcome.data.pending.items[0].arguments.sku | type) == "string"' \
  interaction.json > /dev/null
RUN=$(jq -er '.outcome.data.id' interaction.json)
TOOL_CALL_ID=$(jq -er '.outcome.data.pending.items[0].tool_call_id' interaction.json)
SKU=$(jq -er '.outcome.data.pending.items[0].arguments.sku' interaction.json)
QUANTITY=$(jq -er --arg sku "$SKU" \
  '.[$sku] | select(type == "number" and . >= 0 and floor == .)' inventory.json)
jq -n --arg sku "$SKU" --argjson quantity "$QUANTITY" \
  '{sku: $sku, quantity: $quantity}' > tool-result.json
jq -n --arg id "$TOOL_CALL_ID" --slurpfile result tool-result.json \
  '{answers: [{action: "complete", tool_call_id: $id, result: $result[0]}]}' > resume.json
a13n-service-cli --include-meta runs resume "$RUN" --body @resume.json \
  --idempotency-key 'unique-client-tool-resume-key' > resumed.json
SUCCESSOR=$(jq -er '.data.id' resumed.json)
a13n-service-cli --include-meta runs wait "$SUCCESSOR" --timeout 90 > successor.json
jq '.data.status' successor.json
```

The validation checks the tool name and argument before deriving the result from the matching SKU; an unknown SKU stops without a POST. For a **human approval**, show the pending action to the authorized reviewer and require an explicit decision. This independent branch uses a fresh resume key:

```bash
set -eu
jq -e '.outcome.data.status == "waiting" and
       (.outcome.data.pending.items | length == 1) and
       .outcome.data.pending.items[0].kind == "approval"' interaction.json > /dev/null
jq '.outcome.data.pending.items[0] | {tool_name, arguments, presentation}' interaction.json
RUN=$(jq -er '.outcome.data.id' interaction.json)
TOOL_CALL_ID=$(jq -er '.outcome.data.pending.items[0].tool_call_id' interaction.json)
printf 'Reviewer decision (approve or reject): ' >&2
IFS= read -r decision
case "$decision" in
  approve)
    jq -n --arg id "$TOOL_CALL_ID" \
      '{answers: [{action: "approve", tool_call_id: $id}]}' > decision.json ;;
  reject)
    printf 'Reason for rejection: ' >&2
    IFS= read -r reason
    test -n "$reason"
    jq -n --arg id "$TOOL_CALL_ID" --arg reason "$reason" \
      '{answers: [{action: "reject", tool_call_id: $id, reason: $reason}]}' > decision.json ;;
  *) printf 'No explicit decision; Run remains waiting\n' >&2; exit 2 ;;
esac
a13n-service-cli --include-meta runs resume "$RUN" --body @decision.json \
  --idempotency-key 'unique-approval-resume-key' > resumed.json
SUCCESSOR=$(jq -er '.data.id' resumed.json)
a13n-service-cli --include-meta runs wait "$SUCCESSOR" --timeout 90 > successor.json
jq '.data.status' successor.json
```

A **question-only `user_input` wait accepts no structured answer**. Print the question, obtain the person's reply and send an ordinary message with the Agent and Thread IDs instead:

```bash
set -eu
jq -e '.outcome.data.status == "waiting" and
       (.outcome.data.pending.items | length == 1) and
       .outcome.data.pending.items[0].kind == "user_input"' interaction.json > /dev/null
jq '.outcome.data.pending.items[0] | {tool_name, arguments, presentation}' interaction.json
THREAD=$(jq -er '.submitted.data.thread.id' interaction.json)
AGENT_ID=$(jq -er '.outcome.data.agent_id' interaction.json)
printf "Person's reply: " >&2
IFS= read -r reply
test -n "$reply"
a13n-service-cli --include-meta agents send "$AGENT_ID" --thread "$THREAD" \
  --text "$reply" --idempotency-key 'unique-question-reply-key' --wait > reply.json
jq '.outcome.data.status' reply.json
```

A question-only ordinary message closes the pending question with no-response and starts a successor; it **does not** approve or complete an approval/client tool. Structured resume answers normalize the **entire** pending set: omitted approvals are rejected with "No decision was given", and omitted client results or questions become `no_response`. If multiple actions are pending, explicitly inspect all and construct a deliberate answer batch. The structured resume returns a **distinct successor** Run, so `runs wait "$RUN"` still observes the original waiting Run. An HTTP success or a zero-exit wait is not proof of completion; check the successor status and [committed Items](events-and-readback.md). See [scripting and errors](scripting-and-errors.md) before retrying an uncertain POST.
