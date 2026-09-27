# a13n-service-cli

The companion CLI covers every HTTP operation in the pinned Service API. A generated Clap command tree and typed dispatch call the same `a13n` Rust SDK resources; the binary has no separate HTTP implementation or runtime OpenAPI download. It is an independent Cargo project with its own version and lockfile.

See [CLI workflows](docs/README.md) for installation/build instructions, conditional updates, queued submissions, successor Runs, file transfers and script recovery. This page is the command behavior reference. Both are Markdown maintained in this repository; no separate SDK documentation site is required.

## Start

```bash
export A13N_BASE_URL=https://service.example
export A13N_TOKEN="..." # optional for public endpoints, required by Service for protected routes
export A13N_WORKSPACE=ws_example

a13n-service-cli healthz get
a13n-service-cli agents list --limit 20
a13n-service-cli agents get agt_example --include-meta
a13n-service-cli agents update agt_example --if-match '"etag-from-read"' --body @agent-update.json
a13n-service-cli threads create --idempotency-key new-thread-1 --body @new-thread.json
a13n-service-cli runs resume run_example --idempotency-key resume-1 --body @resume.json
```

Use `--help` at any command depth, `--schema` for its pinned request schema, `--example` for a verified example or an explicitly labelled template, and `completion bash|zsh|fish|powershell|elvish` for offline shell completion. No credentials or Service are required for these discovery commands. Arguments are actual resource names followed by their action; the target ID is positional for leaf operations, while parent IDs use named flags: `threads inbox list --thread thread_id`, `assets content get --asset asset_id --output saved.bin`, `memories files get --memory memory_id --path 'folder/file.txt'`. Workspace/organization contexts always remain explicit, never inferred from tokens. Use root groups such as `organizations workspaces list --organization org_id` where two API scopes would otherwise collide. `--filter-workspace` on an organization catalogue, when present, is a query **filter**, not the command's credential workspace.

JSON body input is inline, `@path` or `@-` (stdin); omission differs from `null`. Unknown object fields in known request schemas are rejected locally, but this is not complete local JSON Schema validation: Service is authoritative. Query/header arguments are generated from the pinned contract, including repeated selectors supplied by repeating their flag and enum values accepted by the SDK. For multipart uploads use `uploads create --idempotency-key key --file path --content-type application/octet-stream` (or `--file - --file-name filename` for stdin). Raw media upload uses `--file` plus the endpoint's content type. Binary downloads require `--output path` or `--output -`, which refuses an interactive terminal; bodies stream through the SDK.

JSON is the default output, independent of TTY. `--include-meta` returns `{"data": ..., "status": 200, "etag": ..., "request_id": ..., "location": ...}`. A 303 retains Location without redirect following; 204 remains `data: null` with actual status when metadata is requested. `--format table` renders readable catalogues and falls back to JSON elsewhere. For cursor collections, `--all` explicitly fetches pages lazily, preserving every complete page body: `{"pages":[page1,page2]}`; with metadata, each array entry is its own `{data,status,etag,request_id,location}` envelope. Without `--all`, one page is returned. A repeated cursor fails instead of looping.

`--dry-run` on generated API commands performs **no HTTP call** and requires no token or trusted CA file; it emits the resolved local method/path, redacted body indication, precondition presence and `server_validated: false`. The `labels` convenience and wait/events helpers explicitly reject `--dry-run` without contacting Service. It does not validate Service state or authorize the request. If-Match and Idempotency-Key are always caller-owned. The CLI does not fetch an ETag and overwrite, follow an unknown mutation outcome, or retry a write.

## Profiles and TLS

Put profiles in `~/.config/a13n-service-cli/config.json` (or set `A13N_CLI_CONFIG` to another JSON file):

```json
{
  "profiles": {
    "dev": {
      "base_url": "https://service.example",
      "workspace": "ws_example",
      "organization": "org_example",
      "ca_bundle": "/path/to/trusted-ca.pem",
      "token_env": "A13N_DEV_TOKEN"
    }
  }
}
```

`--profile dev` selects these values. Explicit CLI flags override the selected profile; ambient `A13N_BASE_URL`, `A13N_WORKSPACE`, `A13N_ORGANIZATION`, and `A13N_CA_BUNDLE` **do not override a named profile**. Without `--profile`, priority is flags > those environment variables > `defaults` in the JSON config. Token bytes are never stored in the config: the named token environment variable (default `A13N_TOKEN`) is read only for network calls. `config show` reports effective nonsecret settings and their sources without printing the token. `--ca-bundle` supplies trusted PEM roots to the SDK transport; no insecure TLS switch exists. `auth login` is a single Service operation, not a persistent interactive authentication system or stored cookie/CSRF session.

## Observation and failure boundaries

`runs wait RUN_ID` stops at that exact Run's completed/waiting/failed/cancelled state. `threads inbox wait ENTRY_ID --thread THREAD_ID` stops at that Entry's consumed/failed/withdrawn state; queued acceptance is not execution or completion, and assignment is not consumption. Neither helper follows a successor Run. `threads events --thread THREAD_ID [--after LAST_APPLIED_CURSOR] [--max-reconnects N]` emits JSONL frames and flushes each line. A cursor-bearing frame becomes applied when the next frame is requested, not merely because it was received; changed/gap/reset are explicit Thread/Run Items readback hints, not reconstructed business state. Save durable applied cursors and checkpointed state in your application, not in this CLI.

`--timeout SECONDS` bounds the whole local request, wait or stream; Ctrl-C returns 130. Neither proves rollback nor stops a durable Run. Human diagnostics never print raw transport errors or request bodies. `--error-format json` writes a machine-readable error object on stderr, with `kind`, `message`, optional HTTP `status`, Service `code` and `request_id`. Exit categories are 2 for input/request, 3 for authentication/permission, 4 for CAS conflict, 5 for transport/protocol/timeout, and 130 for local interruption. Read back a possibly succeeded mutation before explicitly retrying.

The legacy `labels` convenience command remains for Agent, Session, Thread, Run, Skill and EnvironmentTemplate label read/replacement through ordinary SDK GET/PATCH routes; replacement requires an explicit ETag. It does not expose nonexistent legacy `/labels` endpoints.

## Development and release

From the repository root: `make generate` refreshes SDK and CLI-owned generated files from the pinned local contract; `make cli-check-all` checks, tests and builds the CLI. `make check-all` includes packaging and installed-SDK acceptance. Optional live Service CLI acceptance is separate, uses disposable Service state and a trusted CA, and is not proved by local fixtures. Contract sync proposals stage both SDK bindings and generated CLI outputs, never the CLI's handwritten runtime.

The CLI is not packaged inside the `a13n` SDK crate and is not published to crates.io. `release/a13n-service-cli-v<version>` publishes immutable Linux, macOS and Windows x86_64/ARM64 archives and checksums. Licensed under Apache-2.0.
