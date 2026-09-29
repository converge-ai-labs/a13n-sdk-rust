# Learn the Rust SDK

Use the `a13n` SDK to submit work to an existing Service and observe an Agent's exact result. The [quick start](../README.md#get-started-from-source) is a complete, source-installed, **result-only** program: begin there if you have a Service URL, workspace API key and Agent ID. The SDK does not run Agents in-process. The separate [CLI](../a13n-service-cli/README.md) is for shell usage.

## Learning path

1. [Create an Agent and continue a conversation](agent-conversations.md) — select an existing **Model key**, create an Agent, start a Thread, import completed native model history, send a follow-up and use per-Run options. If you already have an Agent ID, start with the quick start and skip creation.
2. [Stream progress and read the final result](streams-and-results.md) — iterate the same finite `Interaction`, then inspect authoritative Run Items; use Thread-wide SSE only when your application really needs it.
3. [Continue a waiting Run](waiting-and-resume.md) — return a complete approval/call result batch, optionally attach ordinary input in the same resume, and observe the **distinct successor** Run; understand queued Entry consumption.
4. [Attach a file and mount Memory](files-and-memory.md) — publish an upload as an Asset, include a structured message part, create a Memory file and mount it into a new Thread.
5. [Authenticate and work with resources](auth-and-resources.md) — choose API-key versus session scope, page through generated collections and conditionally update a disposable Agent with its ETag.
6. [Handle uncertain outcomes without duplicate work](errors-and-recovery.md) — use typed errors and saved receipt IDs to reconcile local timeouts or unknown mutations.

Each chapter states the required Service state and provides a standalone Rust program for a sibling application using the `a13n` and `tokio` dependencies from the quick start. The import and waiting chapters additionally use `serde_json`. `cargo +1.97.0 check` compiles the examples without contacting a Service; running them requires an authorized, configured Service. Example keys are illustrative: a **different** logical POST requires a different caller-owned idempotency key.

## Reference and validation

Use generated Rustdoc or the [pinned OpenAPI](../contract/openapi.json) for exact request fields and permissions. `Client::resources()` covers the complete generated resource graph on the SDK's transport; `Client::execute` also exposes generated HTTP functions for advanced use. The [SDK contract](../spec/README.md) records durable behavior, and [Contributing](../CONTRIBUTING.md) owns local checks. `make accept-installed` verifies an extracted crate with local fixtures; optional disposable HTTPS acceptance is separate. Neither compilation nor local mock tests prove live Service compatibility or publication.
