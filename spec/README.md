# Rust SDK and CLI Contracts

## Ownership and public surface

This repository owns the `a13n` crate, its generated HTTP bindings and adapters, and the separate `a13n-service-cli` executable. Service owns durable state, authorization and wire semantics. The SDK consumes exact local inputs identified by `contract/source.json`; development and generation do not execute Service or import sibling SDKs. SDK version, CLI version and Service source SHA are independent. A new snapshot does not publish either artifact.

`Client::resources()` is the complete generated ordinary resource graph. Collections bind IDs or keys with `at`; binding is local and never expands credential authority. References borrow the owning client. Methods accept the full generated wire models, typed query/header options and explicit mutation preconditions. POST-only action leaves become methods on their owner. Ordinary references, methods and options include generated Rustdoc with HTTP paths and preconditions. Closed inline parameter enums are typed selectors (`ProviderKind`, `MemberKind`, `SkillSource`) with Service wire serialization; they do not change the underlying contract. Run Items is a snapshot read, not a collection.

`Client::execute` is advanced generated HTTP access through the same pool and shutdown lifetime. There is no second handwritten catalogue of routes. Ordinary resources and low-level HTTP operations are both generated from the pinned OpenAPI and use the same generated wire types. The unpublished old Web Provider facade and implicit credential-context lookup are replaced rather than kept as alternate resource trees.

## Models, responses and transport

Wire models live in `generated::models`. Typed model unions retain their branches. Optional nullable fields use double options: omission, explicit null and value stay distinct. Boolean constants remain booleans. Arbitrary JSON uses `serde_json::Value` only where the schema admits it. Unknown Run status strings remain available without opening closed discriminator tags. Models are not complete local JSON Schema validators.

The client supports a Bearer credential, public unauthenticated requests, or a caller-owned session cookie jar and concurrent-safe current CSRF-token callback. Bearer and session configuration cannot be combined. It never infers tenant scope from credentials. Service decides authorization, including restrictions on account operations for workspace-confined keys. Credential diagnostics remain redacted without changing authorized wire serialization.

`Response<T>` retains typed `data`, actual status and headers, including ETag and request ID. OAuth callback completion also preserves its owning semantics' 303 redirect and Location header without following it; the data is the type's default on that bodyless response. `ApiError` exposes status, code, message, details, request ID, retry advice and headers. Ordinary resource JSON is bounded to 16 MiB by default. `Error::Transport` carries safe build/request/body diagnostics based on transport categories, without retaining raw causes, URLs or response bodies. `Error::Protocol` carries a typed violation kind and response status/request ID when available; its formatting and error source omit the untrusted request ID. Neither category implies rollback or retry safety. Closed-client errors and caller-owned timeout/cancellation remain distinct. Low-level generated results keep their own parser, typed errors and raw content, rather than inheriting ordinary resource buffering limits.

Binary and SSE success bodies remain streaming. `BinaryResponse` owns the response; callers drop or close it. Its reads share parent shutdown. Uploads accept an explicitly named and typed owned request body, including reqwest streaming bodies; cancellation or drop releases that body. Image operations require an explicitly allowed MIME type.

The SDK does not impose a whole-response timeout. Callers bound any async operation, including its request, body and polling sleeps, by dropping its future or wrapping the whole future in a timeout. Client close cancels owned local work and releases its pool; neither local cancellation nor transport failure proves Service rollback or stops a durable Run. Uncertain mutations are not automatically retried, even with an idempotency key. Custom HTTP builders configure TLS and connection settings while SDK redirect and no-retry policy is retained.

## Resources and interaction

Conditional mutations require nonempty `if_match`, except Memory revision restore whose target may be absent. Declared request keys are explicit and required. Server validation owns stale preconditions and all resource-level constraints.

Cursor-bearing collections expose lazy `pages` with async `next`. Each page retains HTTP evidence. Options are owned snapshots, requests are not prefetched, and a repeated cursor fails rather than looping. Bounded catalogues and Items snapshots have no fabricated cursor API.

Full-body submission methods return `Submitted`: the response receipt, Thread and Entry references, and an optional Run reference. These use canonical workspace identity from the receipt, not the request's workspace key. A queued entry has no Run. `text_payload` is only a convenience over the full generated message union.

Run `wait` observes exactly that Run until completed, waiting, failed or cancelled; it neither follows successors nor declares business success. Entry `wait` observes consumed, failed or withdrawn; assignment is not consumption. Both share cancellation across requests, body delivery and sleeps. Fork, resume, interrupt and inbox edits retain their generated methods, status and response metadata.

## Thread observation

Thread `events` opens the SSE endpoint and decodes five variants: delta, boundary, changed, gap and reset. Only delta and boundary carry cursor IDs. Envelopes, item references, UTF-8 and bounded framing are validated; AG-UI event contents stay application-owned.

A stream has one mutable reader. Calling the next `next` acknowledges the previously returned cursor frame, not the one about to be delivered. Dropping or closing observation does not acknowledge pending data or mutate the Run. Applied and received cursors are independently visible. Explicit, bounded reconnection sends only applied cursors. Acknowledged new cursor progress resets its failure budget; hint-only traffic does not. Authentication/protocol errors are terminal; transient transport and selected HTTP failures may reconnect within the budget. Parent close stops buffered delivery and recovery delays too.

Gap/reset require authoritative Run Items readback; changed requires Thread readback. Applying that state and maintaining durable application checkpoints are the consumer's responsibility. Cancelling a pending stream read must not silently consume a partial frame or acknowledge undelivered data.

## CLI boundary

The CLI is a complete, generated, API-first companion for every pinned HTTP operation. Generation uses the same local OpenAPI input and ordinary SDK resource graph as the crate to produce a static Clap command catalogue, request schemas and typed SDK dispatch. The executable neither downloads schemas at runtime nor owns a second HTTP client. Each operation has one discoverable `<resource> [subresource] <action>` command (list/get/create/update/replace/delete and named actions such as archive, restore, interrupt and resume). The target resource's ID follows the action where unambiguous; parent context IDs use named flags. Workspace and organization scopes are explicit; a credential never selects one. Explicitly named root groups distinguish otherwise ambiguous account, organization and workspace routes. Labels remains a small convenience command, not a parallel labels endpoint.

`--profile NAME` selects a profile in a local JSON config. CLI flags override it, but ambient target/scope variables never silently override an explicitly selected profile. Without `--profile`, CLI flags override environment values, which override config defaults. Config stores a token **environment variable name**, never token bytes; `config show` reports effective non-secret values and their sources. The CLI permits public no-token operations, uses the SDK's Bearer transport when an environment token is present, and supports an explicit trusted PEM `--ca-bundle` without disabling TLS verification. It does not establish persistent interactive login, cookie/CSRF sessions or a Service deployment lifecycle; `auth login` is just a single HTTP operation.

JSON is the default regardless of terminal detection. `--include-meta` returns `data`, actual status, ETag and request ID, plus Location for a redirect without following it. `--all` is opt-in only for cursor-bearing collections and returns `{"pages": [<complete page bodies>]}` (or a per-page metadata envelope with `--include-meta`) rather than discarding fields or inventing cursor semantics. `--format table` only renders suitable catalogue results. Request JSON accepts inline text, `@file` or `@-`; omission, null and values remain distinct. The CLI rejects unknown fields in known request models rather than silently ignoring misspellings; server validation remains authoritative. Upload accepts a file or stdin and an explicit content type; downloads require an output path or explicit stdout and stream without buffering the whole response.

Help, request `--schema`, command `--example` and shell completion are offline. Generic examples are labelled templates rather than fabricated valid Service requests. For generated API commands, `--dry-run` resolves a local method and target with redacted body/precondition presence without contacting Service or claiming server validation. Labels and observation helpers reject dry-run locally without contacting Service. Caller-owned If-Match and idempotency keys are never inferred or silently replayed; local timeout and interruption cancel only local work, and an uncertain mutation requires readback before an explicit retry. Errors have stable categories and optional structured JSON diagnostics, not raw transport causes or credential values. Run and Entry wait helpers observe precisely the supplied identity; neither follows successors nor promotes acceptance to completion. Thread event JSONL exposes applied cursors and readback hints for gap/reset/changed, not automatic business-state reconstruction.

The CLI owns a separate Cargo workspace, manifest, lockfile and version. Its path dependency points to the containing SDK; it is excluded from the SDK crate. Distribution consists of immutable Linux, macOS and Windows x86_64/ARM64 archives with license and checksums, not a crates.io package.

## Verifiable invariants

- Every pinned HTTP operation has both a low-level binding and a callable ordinary resource method.
- Generation consumes pinned local inputs; generated bindings compile and pass operation dispatch and wire tests.
- Wire tests cover unions, nullable presence, constants, unknown Run status and typed invalid-request rejection.
- Transport tests cover actual status/headers, CAS, streaming ownership, session state, bounded responses and cancellation.
- Wait and SSE tests cover exact identity, queued receipts, applied cursor recovery and shutdown.
- Cargo metadata keeps CLI and SDK independent; the SDK package excludes CLI source.
- Local generation, tests and packaging alone do not prove live Service compatibility or publication.
