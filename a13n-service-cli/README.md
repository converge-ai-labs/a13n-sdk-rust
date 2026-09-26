# a13n-service-cli

`a13n-service-cli` is the cross-platform command-line client for the a13n Service `/api` surface.

## Status

The binary exposes `labels` for Agent, Session, Thread, Run, Skill and EnvironmentTemplate. Every command requires an explicit `--workspace` ID or key. Set `A13N_TOKEN` to a Service API token and pass `--base-url` for the Service origin. Reads print a JSON object containing `labels` to stdout and the resource ETag to stderr. Replacement requires that exact resource ETag and a complete JSON string-to-string map; `{}` clears all labels.

The CLI calls the SDK's workspace-scoped resource GET/PATCH methods. It does not use legacy `/labels` endpoints. Environment resources have no labels in the current contract and are not offered. A general AWS-style command mapping is not implemented; complete SDK coverage does not imply complete CLI coverage.

```bash
a13n-service-cli --base-url https://service.example labels run run_example --workspace ws_example
a13n-service-cli --base-url https://service.example labels agent agt_example --workspace ws_example
a13n-service-cli --base-url https://service.example labels run run_example --workspace ws_example --set '{"project":"support"}' --if-match '"etag-from-read"'
```

The CLI does not implement a separate HTTP client or manage a13n Service processes and infrastructure.

## Development

This directory is an independent Cargo project with its own lock file. From the repository root, run:

```bash
make cli-check
make cli-check-all
```

## Release

`release/a13n-service-cli-v<version>` publishes immutable archives for Linux, macOS, and Windows on x86_64 and ARM64. Releases contain binaries and checksums only; this package is not published to crates.io.

## License

Licensed under the Apache License 2.0.
