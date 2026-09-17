# Contributing

Write code, documentation, commit messages, Issues and pull requests in English. Discuss unresolved product, architecture, compatibility and scope decisions in Issues; accepted design belongs in `spec/`. Use descriptive short-lived branches from `main` and Conventional Commit PR titles. Draft PRs run the full CI gate; mark ready after compatibility review and resolving failures. Resolve review threads and merge through PRs. Repository governance retains the parent's squash-only merge, protected main and immutable release-tag policies.

## Development

Use Rust 1.97.0 with rustfmt and Clippy to match generation and CI, plus Python 3.13, uv and Make. `rust-toolchain.toml` selects that toolchain automatically for local Cargo/rustfmt commands. Update it, `ci.yml`, and `sync-service-contract.yml` together; the manifest's `rust-version` remains the consumer minimum. `make install` resolves locked SDK, CLI and tooling dependencies. The SDK and CLI have separate Cargo manifests, lockfiles and workspaces; the CLI consumes the SDK through the path dependency `..`.

`make format` applies formatting. `make check` verifies both SDK and CLI Clippy plus tooling types/style; `make test` runs SDK, generator and workspace-boundary tests; `make package` verifies the SDK crate. `make cli-check-all` checks, tests and builds the CLI. `make check-all` includes all of these plus non-mutating generated-output verification. These commands require no Service or other SDK checkout.

### Quality gates

Run `make hooks-install` once after `make install` to install pre-commit in this checkout. Commit hooks run file hygiene, Markdown/Ruff formatting, and rustfmt for the affected SDK or CLI. Clippy, Pyright, tests, and generation remain explicit Make/CI checks; no networked generation runs on every commit.

- `make format` applies native, Python, and owned Markdown formatting.
- `make lint` checks SDK formatting/Clippy, Python style, and Markdown without rewriting files.
- `make typecheck` runs Pyright `standard` on Python tooling; native Rust type checking is covered by Clippy and builds.
- `make check` includes the CLI's static checks without merging its independent workspace.
- `make hooks-check` runs all hooks. Formatter changes fail the run for review and restaging; do not bypass hooks.
- `make check-all` runs hooks and the complete existing provenance, generation, static, test, package, and CLI gates. CI uses that same entry point without installing hooks.

Vendored contract evidence is excluded from hooks and Markdown formatting; the locally owned `contract/README.md` is still checked. Native formatting includes generated Rust using the same pinned toolchain as generation, and generated code remains compiled/linted/tested. If generated output drifts, fix its generator rather than staging a manual patch. Keep linting moderate and scoped, as in the parent repository, rather than enabling all optional rules.

Keep changes direct and scoped. Preserve typed unions, omitted/null/value, credential redaction and cancellation semantics. Modify templates/adapters, never generated output by hand. Generated Rust may suppress style/complexity/performance suggestions locally, not correctness/compiler diagnostics or checks on handwritten code. Report exact validation outcomes and reuse still-valid results.

## Releases

Source versions stay `0.0.0`; release workflows inject versions only in ephemeral checkouts. Versions are stable `X.Y.Z` or `X.Y.Z-rc.N`, with positive N and no leading zeroes. Tag a main-line commit whose required CI passed; tags are immutable.

The SDK channel is `release/a13n/rust/<version>` and uses `sdk-rust-crates-io` for crates.io publication. The CLI channel is independently `release/a13n-service-cli-v<version>`; it publishes binary archives and checksums to GitHub, never crates.io. CLI release preparation versions the CLI only; the SDK source dependency remains independent. RC releases do not advance a stable latest pointer. Actual publishing requires separate maintainer authorization and applicable credentials; local packaging does not establish remote publication.

### Release operations

The release workflow verifies that the tagged commit is an ancestor of `origin/main` and that its latest push-triggered `ci.yml` run on `main` completed successfully. It fails rather than waiting for CI or silently using an older successful attempt. Rerun a blocked release only after CI succeeds. This read-only check uses `actions: read`; it does not replace branch protection or rerun the full test suite. Local tests for this boundary require Git, Bash and jq and use a fake GitHub CLI response, not production credentials.

Version preparation and artifact builds operate in ephemeral checkouts. Do not commit their modified manifests/lockfiles. Release tags are immutable, and a retry must retain the same tag and source commit. Publication is not transactional across registry and GitHub: an earlier job may have published before a later job failed. Inspect the registry, tags, artifacts and workflow result before rerunning; do not move tags or assume every publish step is idempotent.

Changelogs use first-parent history scoped to this repository and select only ancestor tags from the same release channel. RCs compare against an earlier RC for the same target, otherwise the preceding stable; stable releases compare against the preceding stable. PR labels classify and omit entries with a Conventional Commit fallback. Curated `.github/release-notes/COMPONENT/VERSION.md` notes are optional. Preview an existing, locally fetched tag without publishing (GitHub PR-label reads still require `gh` authentication):

```bash
GITHUB_REPOSITORY=converge-ai-labs/a13n-sdk-rust \
  python3 scripts/create-github-release.py a13n-rust 1.2.3 "a13n SDK 1.2.3" --dry-run
```

The first channel release uses initial or curated notes rather than attributing extracted monorepo history to this repository's PR numbers. RC GitHub Releases explicitly avoid `latest`. Repository privacy is separate from package visibility: registry publication can expose the package even when its source repository remains private.

The `sdk-rust-crates-io` environment must supply `CARGO_REGISTRY_TOKEN` for `a13n`; copying environment policy does not transfer it. The SDK crate excludes the companion CLI. CLI changelogs select `a13n-service-cli/` and its own workflow/curated notes; SDK changelogs exclude those paths. CLI version injection changes only its manifest and lock entry, preserving its SDK path dependency. Its workflow builds Linux, macOS and Windows x86_64/ARM64 archives, each with the executable and `LICENSE`, plus `SHA256SUMS`. Both stable and RC CLI GitHub Releases use `--latest=false` and require no registry environment.

## Service contract updates

Contract tooling requires Git, Bash, jq and `shasum`. Ordinary builds need no Service checkout or credentials. Generation verifies the local manifest and hashes before reading the snapshot; `contract/README.md` is local guidance, not upstream evidence.

From a clean SDK branch, with the Service repository's full `origin/main` history fetched:

```bash
bash scripts/sync-contract.sh /path/to/agent-foundation FULL_40_CHARACTER_SERVICE_SHA
make generate
make check-all
```

Sync copies Git blobs, never working-tree files or executable Service code. It requires a complete main-line SHA, forward ancestry from the old pin, and byte-accurate old provenance. Missing/malformed inputs fail before writes; same-SHA retries do nothing. Inspect any interrupted local write and restore only the affected snapshot before retrying. The snapshot includes OpenAPI, both wire schemas, fixtures, API conventions, Native streaming and queue semantics. The source compare exposes runtime-only changes too. Follow recorded upstream paths for related specifications; accepted specs take precedence over inconsistent implementation.

`sync-service-contract.yml` receives Service dispatches or a manual full SHA, prepares the SDK toolchain and invokes `open-contract-pr.sh` in an ephemeral checkout. The script selects the existing rolling proposal (or current SDK `main`), copies the requested snapshot and runs `make generate`, then creates or updates a **draft PR** containing the snapshot and generated output. Only `contract/` and `src/generated/` are staged. It never merges, tags or releases.

Generation failure stops before committing or pushing; discard the ephemeral checkout and retry after fixing the cause. There is no contract-only fallback. Full SDK CI runs on drafts, so compilation/test failures remain visible for maintainer adaptation. Review compatibility and generated changes, fix templates or handwritten code as needed, regenerate and resolve CI failures before marking ready.

Each repository has at most one open automatic update PR on `sync/service-contract`. Its snapshot still records the complete immutable Service SHA. The workflow serializes updates; the script additionally compares incoming ancestry against both the accepted `main` pin and the pending proposal, so equal or older notifications never regenerate or rewind newer work. A newer source merges current SDK `main` into the proposal and appends the snapshot/generated changes without force-updating the branch. Handwritten code, templates and reviewer commits are retained; merge conflicts, invalid provenance, failed generation and concurrent pushes stop before replacing remote work. Generated files remain generator-owned, not a place for handwritten adaptation.

The marked source block and PR title track the proposed SHA and its range from the accepted pin. Notes outside that block are preserved. New source revisions return ready PRs to draft and require fresh review; same-SHA retries preserve readiness and only reconcile metadata. If push succeeded before PR creation/editing failed, retry reuses the remote snapshot without regenerating it. Closing an unmerged rolling PR pauses automatic proposals until a maintainer reopens it.

After merge, the next update starts from SDK `main`. Normally GitHub deletes the merged branch; if it remains at exactly the recorded merged head, automation removes it with an exact-head lease only after generation succeeds, then creates the next branch without replacing a concurrently created ref. Any commits added after merge are instead retained and reviewed. A missing branch for an open PR is an error, not permission to discard reviewer work. During migration, inspect old `sync/service-contract-<SHA>` PRs for manual work and close superseded proposals only after the rolling replacement is verified.

### Setup

Install both repositories' workflows on their default branches first. Use a dedicated GitHub App installed only on Service and the four SDK repos, with Contents and Pull requests read/write. Configure variable `SERVICE_CONTRACT_APP_CLIENT_ID` and secret `SERVICE_CONTRACT_APP_PRIVATE_KEY` in those repos (or restrict an organization secret to them). Never copy a developer's OAuth token. Workflows request separate Service-read and destination-write tokens and do not persist checkout credentials. Without a client ID the job is skipped; configuration enables it without another feature flag.

Verify one known main SHA end to end before relying on notifications: dispatch, generation, draft contents, provenance and draft CI. Successful generation is not compatibility acceptance. Offline tests use temporary Git repos and fake GitHub responses; they do not prove App installation or delivery. Registry credentials and release authorization remain separate.
