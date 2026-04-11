# MVL — Minimum Verification Language
.ONESHELL:
SHELL := /bin/bash

.PHONY: help build test lint docs docs-serve tree-sitter-build tree-sitter-test clean

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

# === Assurance ===

assurance: ## Check ISPE traceability: spec → implementation → tests
	@python3 tools/assurance.py

assurance-verbose: ## Assurance dashboard with per-requirement detail
	@python3 tools/assurance.py --verbose

assurance-gate: ## CI gate: fail if below 75% completeness/coverage
	@python3 tools/assurance.py --min 0.75

# === Documentation ===

docs: ## Build documentation site
	uvx --with mkdocs-material mkdocs build

docs-serve: ## Serve documentation locally (http://localhost:8000)
	uvx --with mkdocs-material mkdocs serve

# === Clean ===

# === Tree-sitter (editor support) ===

tree-sitter-build: ## Build tree-sitter grammar for Zed/Neovim
	cd etc/tree-sitter-mvl && npm install && npm run build

tree-sitter-test: ## Run tree-sitter corpus tests
	cd etc/tree-sitter-mvl && npm test

# === Clean ===

clean: ## Clean build artifacts
	cargo clean
	rm -rf build/ site/
