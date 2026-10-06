# Script the CLI without hiding failures

The CLI prints JSON by default, even when stdout is a terminal. Use `--include-meta` when your script needs the real HTTP status, request ID or ETag. Keep each idempotency key with the logical mutation it identifies; if a network error leaves the result unknown, read back before any explicit retry.

## Inspect a request offline

You need the source-built CLI, but not a credential or running Service for help, schema, examples or generated-command dry runs. This example builds a typed plan without sending a POST:

```bash
a13n-service-cli threads create --help
a13n-service-cli threads create --schema > new-thread-schema.json
cat > new-thread.json <<'JSON'
{"agent_id":"replace-with-agent-id","message_history":[{"kind":"request","parts":[{"part_kind":"user-prompt","content":"Earlier question"}]}],"payload":{"content":[{"type":"text","text":"Hello"}]}}
JSON
a13n-service-cli --base-url 'https://service.example' --dry-run threads create \
  --body @new-thread.json --idempotency-key 'dry-run-only-key' > plan.json
cat plan.json
```

The generated `threads create --schema` exposes `message_history`; `runs resume --schema` exposes the required `approvals`/`calls` maps and optional `input`. Their offline plans still redact body content. The plan shows a local method/path and `server_validated: false`; it does not establish that the Agent exists, that the key is authorized, or that Service validation will pass. `--example` returns a verified example or a **labelled template** when a ready-made valid example is unavailable. The CLI checks unknown object fields in known JSON bodies but is not a complete JSON Schema validator. The `labels` convenience and wait/events helpers reject `--dry-run` rather than claiming to preview remote state.

## Handle stdout, stderr and exit status separately

When configured with a real Service, `--include-meta` returns an envelope such as `{ "data": ..., "status": 200, "etag": ..., "request_id": ..., "location": ... }`. Use `.data.id` rather than `.id`. With `--all --include-meta`, `pages` contains one full envelope **per** cursor page; without `--all`, a list command fetches just one page. HTTP `204` has `data: null`; a `303` keeps its `Location` and is not followed automatically.

For an offline input-error demonstration, omit the required Agent ID. This should exit 2 without a Service request; the error is on stderr, never mixed with response JSON:

```bash
set +e
a13n-service-cli --error-format json agents get > response.json 2> error.json
status=$?
set -e
printf 'Exit: %s\n' "$status"
cat error.json
```

The CLI uses exit 2 for input/request errors, 3 for authentication/permission, 4 for conditional-update conflicts, 5 for transport/protocol/timeout and 130 for Ctrl-C. `--error-format json` supplies a safe `kind`/`message` and, when available, Service `status`, `code` and `request_id`. Do not log token bytes or raw request bodies. `--timeout SECONDS` bounds a local call, wait or stream; neither timeout nor Ctrl-C proves that remote work stopped or a POST rolled back.

`ThreadView.last_run_id` is the most recently sealed Run of any outcome. Completed/failed/cancelled history continues through a new explicit normal `agents send` message; failure/cancellation pauses automatic advancement, not continuation history. Resume is only for the exact idle last waiting Run, not an all-status retry or implicit fork.

The same uncertain-outcome rule applies to a `runs resume` with its complete result maps and optional input: retain the exact waiting Run ID and idempotency key, and reconcile the successor rather than changing the batch on retry. If an `agents start`/`agents send` command may have succeeded before a response was lost, keep its original key and saved receipt IDs. Observe the **Entry for that submission**; only a consumed Entry's `assigned_run_id` identifies its incorporating Run. Do not use a Thread's latest Run as a shortcut and do not resubmit a *different* message with the old key. See [Agent workflows](agent-workflows.md#a-message-queued-behind-another-run) and [stream readback](events-and-readback.md). Generated operations preserve explicit ETags and idempotency keys; the CLI never silently fetches a new ETag to overwrite state, retries a mutation or resumes pending human actions.
