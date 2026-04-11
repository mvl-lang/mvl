# MVL — Minimum Verification Language
.ONESHELL:
SHELL := /bin/bash

.PHONY: help build test lint docs docs-serve clean

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

# === Setup ===

setup: ## Install git hooks and verify tooling
	git config core.hooksPath .githooks
	@echo "Git hooks installed from .githooks/"
	@command -v cargo >/dev/null 2>&1 || { echo "cargo not found — install Rust: https://rustup.rs"; exit 1; }
	@echo "Ready."

# === Build ===

build: ## Build the MVL compiler
	@echo "Building MVL compiler..."
	cargo build

build-release: ## Build release binary
	cargo build --release

# === Test ===

test: ## Run all tests
	@echo "Running all tests..."
	cargo test

test-unit: ## Run unit tests only
	cargo test --lib

test-integration: ## Run integration tests
	cargo test --test '*'

test-corpus: ## Validate corpus examples parse and type-check
	@echo "Validating corpus examples..."
	@for f in tests/corpus/**/*.mvl; do \
		echo "  $$f"; \
		cargo run -- check "$$f" || exit 1; \
	done
	@echo "All corpus examples valid."

# === Quality ===

lint: ## Lint with clippy
	cargo clippy -- -D warnings

format: ## Format code
	cargo fmt

format-check: ## Check formatting without changing files
	cargo fmt -- --check

# === Documentation ===

docs: ## Build documentation site
	uv run mkdocs build

docs-serve: ## Serve documentation locally (http://localhost:8000)
	uv run mkdocs serve

# === Clean ===

clean: ## Clean build artifacts
	cargo clean
	rm -rf build/ site/
