# Rust SDK and CLI Contracts

## Ownership

This repository owns the `a13n` crate, its generated HTTP bindings and adapters, and the `a13n-service-cli` executable. Service owns durable state, authorization and wire protocol semantics. The SDK consumes exact local inputs identified by `contract/source.json`; development and generation do not invoke Service or sibling SDKs.

The SDK release version, CLI release version and Service source SHA are independent identities. Contract updates require review of generated/public API compatibility before an SDK release. A new snapshot does not itself publish either artifact.

## SDK HTTP boundary

Generated bindings cover ordinary Native `/api/v1` HTTP operations in the pinned OpenAPI. Typed model unions retain their branches. Optional nullable fields use double options: omission, explicit null and value stay distinct. Boolean constants remain booleans; optional null-only fields preserve presence. Unconstrained JSON branches use `serde_json::Value` only where the source admits arbitrary JSON. Unknown Run status strings remain available without opening closed discriminator tags.

`Client.execute` borrows generated configuration over the parent reqwest pool and cancellation token. Workspace bindings share that parent lifetime and do not expand credential authority. Separately constructed generated configurations have independent ownership. JSON responses preserve typed data plus status/headers; generated errors retain status, headers, raw content and typed entities. These results do not inherit the Web facade's bounded response or error mapping.

Binary responses remain streaming; callers consume them within `execute` so parent close covers delivery. File upload operations stream from caller-supplied paths. Dropping a future cancels its local request; closing the client cancels owned work. Neither cancellation nor a transport failure after possible dispatch proves Service rollback. Mutations are not automatically replayed after uncertain outcomes.

Credential-bearing diagnostics remain redacted without changing authorized wire serialization. Generated HTTP bindings do not promise full JSON Schema validation, Run SSE or notification WebSocket recovery.

## CLI boundary

The CLI uses the Rust SDK for every network operation. It has no parallel HTTP client and no Service process, deployment or infrastructure responsibilities. Its commands expose only their implemented SDK journeys; broad generated HTTP coverage does not imply equivalent CLI command coverage.

The CLI owns a separate Cargo workspace, manifest, lockfile and version. Its path dependency points to the containing SDK; it is excluded from the SDK crate. CLI distribution consists of immutable Linux, macOS and Windows x86_64/ARM64 binary archives with license and SHA-256 checksums. The CLI is not a crates.io package.

## Verifiable invariants

- Every pinned HTTP operation has a binding; local source hashes match metadata.
- Generation checks compare names and bytes without refreshing committed output.
- Wire tests cover union branches, nullable presence, constants, decimal values and unknown Run status.
- Transport tests preserve headers, response evidence, streaming and cancellation; compile-fail examples reject invalid request types.
- Cargo metadata keeps the CLI and SDK in independent workspaces; the SDK package excludes CLI source.
- CLI tests exercise commands through the SDK rather than a second network implementation.
