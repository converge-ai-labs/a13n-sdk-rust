# a13n-service-cli

Use this command-line companion to ask an existing a13n Service Agent to work and read the result. It is a separate pre-public source project, not a published binary or part of the SDK crate.

## Build and connect

Build from this repository's root with Rust 1.97.0, then add the resulting executable to your PATH (or invoke it at the path shown):

```bash
rustup toolchain install 1.97.0
cargo +1.97.0 build --manifest-path a13n-service-cli/Cargo.toml --locked --release
export PATH="$PWD/a13n-service-cli/target/release:$PATH"
a13n-service-cli --help
```

You need an existing Service URL, a workspace API key authorized for an existing Agent, and the **Agent ID** returned by the Service. The API key selects its own workspace; do not set `A13N_WORKSPACE` for ordinary key calls. Never save the token in a profile or committed file:

```bash
export A13N_BASE_URL='https://your-service.example'
export A13N_TOKEN='your-workspace-api-key'
AGENT_ID='your-existing-agent-id'
```

If the Service uses a private CA, configure [trusted TLS roots](docs/connection-and-profiles.md) first; the CLI has no insecure verification switch.

## Ask an Agent

Choose a unique idempotency key for this logical request. `--wait` observes the submission through its exact Run and prints both acceptance and the final status:

```bash
a13n-service-cli --include-meta agents start "$AGENT_ID" \
  --text 'Summarize this project' --idempotency-key 'a-unique-request-key' \
  --wait > interaction.json
cat interaction.json
```

The JSON contains `.submitted.data.thread.id` and `.outcome.data.status`. Completion, waiting, failure and cancellation are distinct statuses; do not assume a successful command exit means the Agent completed successfully. A failed/withdrawn queued Entry instead produces a structured error. For a follow-up, supply the returned Thread ID and another request key:

```bash
# Requires jq; see docs/README.md for a script without hidden state changes.
THREAD=$(jq -er '.submitted.data.thread.id' interaction.json)
a13n-service-cli agents send "$AGENT_ID" --thread "$THREAD" \
  --text 'Explain the trade-off' --idempotency-key 'another-unique-request-key' \
  --wait
```

See [CLI workflows](docs/README.md) for queued submissions, readback, streaming, conditional updates and transfers. The [Rust SDK](../README.md) offers result-only and finite frame iteration in application code.

## Learn by task

- [Connect and configure a profile](docs/connection-and-profiles.md) with an API key, private CA if necessary, and nonsecret saved settings.
- [Start, continue and wait](docs/agent-workflows.md) for one Agent submission, or explicitly follow its queued Entry and successor Run.
- [Watch Thread events and read committed Items](docs/events-and-readback.md); events are Thread-wide and Ctrl-C only stops local observation.
- [Transfer files and update with ETags](docs/files-and-cas.md), keeping uploads, Assets and Memory files distinct.
- [Script safely and interpret failures](docs/scripting-and-errors.md), including offline help/schema/dry-run, response envelopes and retry boundaries.

All pinned Service operations are available through generated commands on the same SDK transport; no separate CLI HTTP implementation or runtime contract download is needed. `--help` at any command depth, `--schema`, labelled `--example`, and shell completion work offline. JSON is the default output; `--include-meta` includes actual HTTP status, ETag, request ID and redirect Location. The CLI never silently retries mutations, follows successor Runs, grants approval or ignores ETag conflicts. `--timeout` and Ctrl-C stop local work, not remote execution.

## Development and release

From the repository root: `make generate` refreshes SDK and CLI-owned generated files from the pinned local contract; `make cli-check-all` checks, tests and builds the CLI. `make check-all` includes packaging and installed-SDK acceptance. Optional live Service CLI acceptance is separate, uses disposable Service state and a trusted CA, and is not proved by local fixtures. Contract sync proposals stage both SDK bindings and generated CLI outputs, never the CLI's handwritten runtime.

The CLI is not packaged inside the `a13n` SDK crate and is not published to crates.io. `release/a13n-service-cli-v<version>` publishes immutable Linux, macOS and Windows x86_64/ARM64 archives and checksums. Licensed under Apache-2.0.
