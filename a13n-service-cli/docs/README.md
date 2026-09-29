# Learn the CLI

Use `a13n-service-cli` to invoke an Agent on an existing Service from your shell. The [CLI quick start](../README.md#build-and-connect) builds the pre-public source, configures an API key and shows `agents start --wait` followed by `agents send`. Start there; no published binary is assumed. Examples here use a POSIX shell and `jq` where noted. Replace sample URLs, IDs and request keys with your own.

## Follow a task

1. [Connect and configure profiles](connection-and-profiles.md) — choose URL, API-key environment variable and trusted CA without storing token bytes in JSON. An API key already selects its workspace.
2. [Start, continue and wait for an Agent](agent-workflows.md) — import native model history on Thread creation, observe one submission through its consumed Entry, and resume an exact waiting Run with complete results and optional input.
3. [Watch events and read committed Items](events-and-readback.md) — stream Thread-wide JSONL, persist only applied cursors and read authoritative Run Items after a gap or reset.
4. [Transfer files and update safely](files-and-cas.md) — publish binary uploads as Assets, edit text Memory files and use current ETags for conditional updates.
5. [Script without hiding failures](scripting-and-errors.md) — inspect command schemas offline, parse result metadata, distinguish nonzero exit categories and reconcile uncertain writes.

Use `a13n-service-cli --help` and the relevant leaf command's `--help` for exact flags. Generated API commands also offer offline `--schema`, `--example` and `--dry-run`. The Service remains authoritative for access control and request validation; a local plan is not a successful call. For application code and per-submission finite frame iteration, see the [Rust SDK guide](../../docs/README.md). The [SDK contract](../../spec/README.md) records cross-layer behavior, while [Contributing](../../CONTRIBUTING.md) owns local development checks.
