# agent-foundation

`agent-foundation` is the cross-platform command-line client for the Agent Foundation Service `/api` surface.

## Status

The initial binary exposes only `--help` and `--version`. Network commands are added only when the corresponding service API, typed Rust SDK operation, and end-to-end behavior exist. The CLI does not implement a separate HTTP client or manage Foundation Service processes and infrastructure.

## Development

This directory is an independent Cargo project with its own lock file. From the repository root, run:

```bash
make foundation-cli-check
make foundation-cli-check-all
```

## Release

`release/foundation-cli-v<version>` publishes immutable archives for Linux, macOS, and Windows on x86_64 and ARM64. Releases contain binaries and checksums only; this package is not published to crates.io.

## License

Licensed under the Apache License 2.0.
