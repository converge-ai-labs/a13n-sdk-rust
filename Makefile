.DEFAULT_GOAL := help
.PHONY: help install hooks-install hooks-check generate generated-check format lint typecheck test build package check check-all cli-check cli-test cli-build cli-check-all

help:
	@echo 'install | hooks-install | hooks-check | generate | generated-check | format | lint | typecheck | check | test | build | package | cli-check-all | check-all'

install:
	uv sync --locked
	cargo fetch --locked
	cargo fetch --locked --manifest-path a13n-service-cli/Cargo.toml

hooks-install:
	uv run --locked pre-commit install

hooks-check:
	uv run --locked pre-commit run --all-files --show-diff-on-failure

generate:
	bash scripts/sync-contract.sh --check
	uv run --locked python codegen/generate.py

generated-check:
	bash scripts/sync-contract.sh --check
	uv run --locked python codegen/generate.py --check

format:
	git ls-files -z -- '*.md' ':!:contract/semantics/**' | xargs -0 uv run --locked mdformat --number
	cargo fmt
	cargo fmt --manifest-path a13n-service-cli/Cargo.toml
	uv run --locked ruff check --fix .
	uv run --locked ruff format .

lint:
	git ls-files -z -- '*.md' ':!:contract/semantics/**' | xargs -0 uv run --locked mdformat --check --number
	uv run --locked pre-commit validate-config
	cargo fmt -- --check
	cargo clippy --all-targets --all-features --locked -- -D warnings
	uv lock --check
	uv run --locked ruff check --no-fix .
	uv run --locked ruff format --check .

typecheck:
	uv run --locked pyright

test:
	cargo test --all-features --locked
	uv run --locked python -m pytest

build:
	cargo build --all-features --locked

package:
	cargo package --locked --allow-dirty

cli-check:
	cargo fmt --manifest-path a13n-service-cli/Cargo.toml -- --check
	cargo clippy --manifest-path a13n-service-cli/Cargo.toml --all-targets --all-features --locked -- -D warnings

cli-test:
	cargo test --manifest-path a13n-service-cli/Cargo.toml --all-features --locked

cli-build:
	cargo build --manifest-path a13n-service-cli/Cargo.toml --all-features --locked

cli-check-all: cli-check cli-test cli-build

check: lint typecheck cli-check

check-all: install hooks-check generated-check check test build package cli-check-all
