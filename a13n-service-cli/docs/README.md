# CLI workflows

The [CLI README](../README.md) owns flags, profiles, output and exit-code semantics. This guide shows how to combine commands without hiding Service state transitions. Examples use a POSIX shell and, where shown, `jq`; use your shell's quoting rules on Windows. All IDs, URLs and request keys are examples to replace, not provisioned resources.

## Install or build

For a published version, select your operating system/architecture from this repository's [GitHub releases](https://github.com/converge-ai-labs/a13n-sdk-rust/releases), verify the supplied checksum and place `a13n-service-cli` on your PATH. SDK crate releases and CLI executable releases are separate; do not assume every SDK release contains a CLI archive.

To build this checkout, run from the repository root:

```bash
cargo build --manifest-path a13n-service-cli/Cargo.toml --locked --release
./a13n-service-cli/target/release/a13n-service-cli --help
```

The CLI calls an existing Service. It is not `a13n-service` (the server/operator executable), `a13n-harness-ui` (the local agent application), or a deployment manager. A successful source build is not publication.

## Discover before dispatch

```bash
a13n-service-cli --help
a13n-service-cli threads create --help
a13n-service-cli threads create --schema
a13n-service-cli threads create --example
a13n-service-cli completion bash
```

These commands work offline. Schema output is the pinned request schema, and an example may be explicitly labelled as a template. The running Service remains authoritative for authorization, state and validation. Use `--dry-run` only on generated API commands; labels and observation helpers reject it locally.

Configure [profiles and trusted CA roots](../README.md#profiles-and-tls) or explicit environment variables. Do not put token bytes in config JSON or command history. A named profile is isolated from ambient URL/workspace/organization/CA variables. `config show` reports effective nonsecret values and their sources.

## Read and conditionally update an Agent

With the URL, token and workspace configured, retain metadata from the read:

```bash
AGENT=ap_example
a13n-service-cli agents get "$AGENT" --include-meta > agent-before.json
ETAG=$(jq -er '.etag' agent-before.json)

a13n-service-cli agents update "$AGENT" \
  --if-match "$ETAG" --body '{"name":"Reviewer"}' --include-meta
```

The ETag includes quotes; pass it unchanged. On a stale-precondition failure, inspect current state and reconcile rather than automatically replacing the ETag. Request bodies distinguish omission from null, but null does not universally clear fields. Use an explicit empty string when that field's Service definition calls for one.

Metadata output is an envelope containing `data`, `status`, `etag`, `request_id` and `location`. Do not accidentally parse `.id` where `.data.id` is required. With `--all --include-meta`, `.pages` contains one envelope per page; without `--all`, a collection command fetches one page.

## Submit, handle queueing and read committed output

Create `new-thread.json` using an existing Agent ID:

```json
{
  "agent_id": "ap_example",
  "payload": {"content": [{"type": "text", "text": "Review this change"}]}
}
```

Retain one unique key for this logical submission; reuse it only when reconciling/retrying the same request, not for another message:

```bash
REQUEST_KEY=review-unique-request-1
a13n-service-cli threads create --body @new-thread.json \
  --idempotency-key "$REQUEST_KEY" --include-meta > submitted.json

THREAD=$(jq -er '.data.thread.id' submitted.json)
ENTRY=$(jq -er '.data.entry.id' submitted.json)
RUN=$(jq -r '.data.run.id // empty' submitted.json)
```

A missing Run is queued input, not failed acceptance. Observe the Entry explicitly:

```bash
if [ -z "$RUN" ]; then
  a13n-service-cli threads inbox wait "$ENTRY" --thread "$THREAD" \
    --timeout 60 --include-meta > entry.json || exit $?
  RUN=$(jq -er 'select(.data.status == "consumed") | .data.assigned_run_id' entry.json) || {
    printf 'Entry did not produce a consumed Run; inspect entry.json\n' >&2
    exit 1
  }
fi
```

Failed or withdrawn entries do not imply execution; assignment alone is not consumption. The script selects an assigned Run only after checking consumption. Once you have the intended Run ID:

```bash
a13n-service-cli runs wait "$RUN" --timeout 60 --include-meta > run.json
jq '.data.status' run.json
a13n-service-cli runs items get --run "$RUN" > items.json
```

Wait returning successfully is not proof of agent success: it also returns waiting, failed and cancelled states. Each CLI invocation has its own timeout; your script owns a total multi-command deadline.

Resume needs caller-provided answers for the actual pending interactions, plus an idempotency key:

```bash
a13n-service-cli runs resume "$RUN" --body @resume.json \
  --idempotency-key resume-unique-request-1 --include-meta > resumed.json
SUCCESSOR=$(jq -er '.data.id' resumed.json)
a13n-service-cli runs wait "$SUCCESSOR" --timeout 60
```

Waiting on the old Run still observes the old Run. Fork similarly creates a new Thread/Entry/optional Run receipt. Do not fabricate tool results or approvals merely to make a Run continue.

## Follow events without confusing observation with execution

```bash
a13n-service-cli threads events --thread "$THREAD" --timeout 120 > events.jsonl
```

Each JSONL line is flushed before the CLI advances to the next frame. Delta/boundary cursors can be used with `--after`; changed/reset/gap require explicit Thread or Run Items readback. The CLI considers delivery to stdout applied, not your downstream consumer's durable business transaction. A consuming program must save its own last durably applied cursor and state; copying the last received cursor without applying the frame can lose application updates.

Ctrl-C exits 130 and ends local attachment, not the remote Run. Timeout or a broken connection does not prove rollback. Use `runs interrupt RUN_ID` only when you intend the remote command, and read back its outcome.

## Files and Memory

Asset uploads and downloads are binary operations:

```bash
a13n-service-cli uploads create --file ./report.bin \
  --content-type application/octet-stream --idempotency-key upload-unique-1
# Publish the returned upload_id with assets create before downloading its Asset.
a13n-service-cli assets content get --asset asset_example --output ./download.bin
```

For stdin use `--file - --file-name report.bin`; raw image uploads also need the endpoint's explicit image content type. Binary stdout requires `--output -` and refuses an interactive terminal.

Memory files instead contain JSON text and use logical paths, not local disk filenames:

```bash
a13n-service-cli memories files get --memory mem_example \
  --path 'project/notes #1.md' --include-meta

a13n-service-cli memories files replace --memory mem_example \
  --path 'project/notes #1.md' --if-match '"file-etag-from-read"' \
  --body '{"content":"Updated note"}'
```

Quote paths but do not URL-encode them yourself. Memory revision selectors are integers. `memories revisions restore SEQ --memory MEMORY_ID` undoes the selected change; use the current file ETag when present, and expect that undoing creation may return `file: null`. Record-provider operations and Thread memory mounts are distinct from file content.

## Script failures deliberately

Use `--error-format json` for structured errors on stderr, separate from stdout results. Exit 2 is input/request, 3 authentication/permission, 4 CAS conflict, 5 transport/protocol/timeout and 130 interruption. Inspect Service status/code/request ID where present; do not treat every nonzero exit as safe to repeat.

After an unknown mutation outcome, read back using saved IDs and the original request key before explicitly retrying. Generated-command dry-run is a no-network local plan, not evidence of Service authorization or accepted state. See [the SDK application guide](../../docs/README.md) for the underlying resource boundaries.
