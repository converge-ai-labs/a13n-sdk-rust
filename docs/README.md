# Rust SDK application guide

Use `a13n` to call an existing Service from async Rust. It is not an in-process agent runtime. The [README](../README.md) contains compile-checked examples; this guide covers integration decisions and resource semantics. The remote [CLI](../a13n-service-cli/README.md) has its own executable, configuration and release channel.

## Setup and reading map

The consumer minimum is Rust 1.96; repository development uses the toolchain pinned in `rust-toolchain.toml`. Use a released `a13n` crate version appropriate to your Service when available. For a source checkout, an application can declare `a13n = { path = "/absolute/path/to/a13n-sdk-rust" }` in its Cargo dependencies. Source version `0.0.0` is not a registry release. Async examples require a Tokio runtime.

Prepare a Service base URL, an authorized credential, a workspace ID or key and an existing Agent ID. References bind locally; they do not discover resources or broaden credentials. A workspace API key cannot perform every organization/session operation merely because a generated method exists.

| Task                                             | Read                                                                            |
| ------------------------------------------------ | ------------------------------------------------------------------------------- |
| Submit, observe a queued Entry and wait on a Run | [Submit and wait](../README.md#submit-and-wait)                                 |
| Answer pending interactions                      | [Resume a waiting Run](../README.md#resume-a-waiting-run)                       |
| Use CAS, nullable fields and pagination          | [Read, update and paginate](../README.md#read-update-and-paginate)              |
| Follow typed provisional output                  | [Observe a Thread](../README.md#observe-a-thread)                               |
| Inspect structured failures                      | [Handle errors explicitly](../README.md#handle-errors-explicitly)               |
| Access low-level generated methods               | [Advanced HTTP access](../README.md#advanced-generated-http-access)             |
| Contribute or validate                           | [Contributing](../CONTRIBUTING.md)                                              |
| Understand guarantees and provenance             | [Specification](../spec/README.md) and [pinned contract](../contract/README.md) |

## Client and I/O ownership

`Client::new(base_url, Secret::new(token))` uses Bearer authentication. For public access, build without credentials. Session mode uses a caller-owned cookie jar and CSRF callback; it cannot be combined with Bearer credentials. Manage session state explicitly rather than expecting the SDK to sign in interactively.

Use `.http_builder(...)` for TLS/connection configuration, including a private CA. Preserve certificate verification. The SDK has no whole-response deadline; bound the entire request, wait or stream future with Tokio timeout or cancellation. A stream may live much longer than a normal JSON request.

Resource references borrow the Client. Dropping or timing out a future cancels local work, and `client.close()` cancels SDK requests and body delivery. Neither operation proves rollback or stops a durable Run.

Binary responses are unbuffered `BinaryResponse` values. Read `.chunk().await?` until `None`, or close/drop the body when stopping early. Uploads own their request body, filename and MIME; a `reqwest::Body` can stream it. Images require an explicit allowed content type. Ordinary resource JSON/error bodies have a configurable size bound; low-level generated parsing has different rules.

## Find an operation

`client.resources()` exposes the generated resource graph. Scope with `.workspaces().at(workspace_id)` or `.organizations().at(organization_id)`, then select the child. `.at(...)` is local; the leaf async method performs I/O.

| Task                     | Resource path                                            |
| ------------------------ | -------------------------------------------------------- |
| Manage an Agent          | `workspace.agents().at(agent_id)`                        |
| Submit to a Thread       | `workspace.threads().at(thread_id).inbox()`              |
| Read committed Run Items | `workspace.runs().at(run_id).items().get()`              |
| Read a Memory file       | `workspace.memories().at(memory_id).files().at(path)`    |
| Select a Memory revision | `workspace.memories().at(memory_id).revisions().at(seq)` |
| Manage record providers  | `organization.memory_providers()`                        |

Use IDE completion and `cargo doc --no-deps --open` in the checkout for native API documentation. [Generated resources](../src/generated/resources.rs) include per-method routes, options and return types; [generated models](../src/generated/models) contain wire bodies and unions. The [pinned OpenAPI](../contract/openapi.json) defines the snapshot's fields. Neither documentation browsing nor ordinary runtime requests require downloading a current schema.

`Response<T>` preserves `.data`, `.status`, `.headers`, `.etag()` and `.request_id()`. `Submitted` keeps this evidence in `.receipt` alongside bound references. Cursor collections expose `pages(...)`; each `next().await?` retains a complete page response. Bounded catalogues and Run Items are not artificial cursor collections.

## Omission, null and conditional edits

For generated fields typed `Option<Option<T>>`, `None` omits the field, `Some(None)` sends JSON null and `Some(Some(value))` sends a value. Do not assume every optional field has that nested type; use its generated declaration. Service defines null semantics per field. In Agent metadata updates, a null description preserves the old value, while an empty string is an explicit value.

Pass the ETag of the object actually being changed. File content uses a file ETag; Thread inbox/mount changes use the Thread ETag. On a stale precondition, re-read and reconcile your intended edit before explicitly retrying. Fetching a new ETag and overwriting automatically defeats concurrency control.

## Acceptance and observation

- Submission always retains a Thread and inbox Entry. An absent `.run` means queued intent, not rejected acceptance.
- Entry waiting stops at consumed, failed or withdrawn. Assignment alone is not consumption; inspect the result before selecting an assigned Run.
- Run waiting stops at completed, waiting, failed or cancelled. Inspect the status and committed Items; a successful future is not an assertion that the agent succeeded.
- Use one `timeout_at` deadline when queue observation and Run waiting share a total budget, as in the README.
- Resume returns a successor `Response<RunView>`; select its ID explicitly. Waiting on the original Run never follows it. Fork returns a new submission receipt.

Keep caller-owned idempotency keys with logical requests. After an uncertain mutation, reconcile using saved keys and identities before retrying. The SDK never retries a write automatically. Local cancellation is distinct from an explicit remote `interrupt()`.

## Thread streams and recovery

`thread.events(StreamOptions { ... })` yields typed frames; `thread.stream().get(...)` exposes one raw SSE body. Use the typed form when you want the SDK's framing and bounded reconnect behavior.

Only delta/boundary frames have cursors. Polling the next `next()` acknowledges the previously returned cursor frame, so finish applying it first. `applied_cursor()` and `last_received_cursor()` are intentionally different. Closing does not acknowledge pending data. Persist durable checkpoints yourself, together with application state.

A changed hint calls for Thread readback; gap/reset identify a Run whose committed Items must be read and applied. EOF is not Run completion. Authentication/protocol failures do not become endless reconnect loops. A stream has one mutable reader, and dropping a pending read future preserves its partial framing state for later reads.

## Memory files, records and mounts

File Memory stores JSON text, not binary Asset content. Pass logical file paths directly to `.files().at(path)` without pre-encoding them. Files support list/create/get/replace/delete/move with explicit preconditions for existing content.

Revision selectors are integers. Restore undoes the selected revision's change, not necessarily its post-change content. Undoing creation can return `MemoryFileState { file: None }`. `MemoryRevisionRestoreOptions.if_match` uses the current file's ETag when it exists; omit it only for an absent restore target. Purging revision history is not deleting the current file.

Provider-backed `records()` supports list/create/replace/delete/search, not item GET or file-style CAS. Reconcile uncertain provider writes instead of automatically replaying them. Organization `memory_providers()` configures and tests provider accounts.

`thread.memories()` mounts memories by name using the Thread ETag. New Thread and Fork bodies accept mounts; `RunView.memory_mounts` is the accepted snapshot and does not track later Thread changes.

## Diagnose failures

Use `Error::Api` for HTTP status, Service code/details and request ID. Distinguish missing/invalid credentials (`401`), permissions/session proof (`403`), resource/idempotency conflicts (`409`) and missing/stale preconditions (`428`/`412`).

`Error::Transport` reports a safe stage/category; `Error::Protocol` reports a violated response boundary with status/request ID when available. Raw transport causes, URLs and response bodies are not retained in their diagnostic chains. `Connect` does not promise to distinguish DNS from TLS. Inspect structured fields rather than parsing `Display` text. Tokio timeout is an outer error, distinct from an SDK failure.

A transport/protocol failure can follow a committed write. Do not infer rollback from it, and do not log secrets or full request bodies to investigate.

## Compatibility and validation

Read these Markdown guides at the tag or commit you consume. [contract/source.json](../contract/source.json) identifies the Service snapshot; SDK/CLI versions are independent of that pin. Structural coverage of the snapshot does not imply all operations, Service versions or external providers were exercised live.

The README's Rust examples are Rustdoc checks. `make check-all` also covers installed-crate acceptance and the separate CLI. [Opt-in disposable HTTPS acceptance](../CONTRIBUTING.md#disposable-service-acceptance) is a different gate; its documented SSE bridge limitation is not a production TLS recommendation. No local test or documentation change constitutes a release.
