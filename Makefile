# MVL — Minimum Verification Language
.ONESHELL:
SHELL := /bin/bash

.PHONY: help version build build-release test test-unit test-integration test-corpus test-stdlib test-transpiler test-llvm test-llvm-all test-tree-sitter test-grammar-coverage coverage lint mvl-lint format format-check assurance assurance-verbose assurance-gate docs docs-serve tree-sitter-build install install-nvim doctor clean

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

version: ## Show current project version
	@grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/'

# === Setup ===

setup: ## Install git hooks, verify tooling, and install tree-sitter npm deps
	git config core.hooksPath .githooks
	@echo "Git hooks installed from .githooks/"
	@command -v cargo >/dev/null 2>&1 || { echo "cargo not found — install Rust: https://rustup.rs"; exit 1; }
	@command -v node >/dev/null 2>&1 || { echo "node not found — install Node.js: https://nodejs.org"; exit 1; }
	cd etc/tree-sitter-mvl && npm install
	@echo "Ready."

doctor: ## Check that all dev tools are available
	@echo "Checking dev tools..."; echo; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	check() { command -v "$$1" >/dev/null 2>&1 && printf "  $$OK $$1\n" || printf "  $$FAIL $$1  ($$2)\n"; }; \
	check cargo         "https://rustup.rs"; \
	check rustfmt       "rustup component add rustfmt"; \
	check clippy-driver "rustup component add clippy"; \
	check node          "https://nodejs.org"; \
	check python3       "required for make assurance"; \
	check /opt/homebrew/opt/llvm/bin/lli "brew install llvm  (required for LLVM backend)"; \
	echo

install: build-release ## Install mvl binary to ~/.local/bin
	@mkdir -p ~/.local/bin
	cp target/release/mvl ~/.local/bin/mvl
	@echo "Installed: ~/.local/bin/mvl"

# === Build ===

build: ## Build the MVL compiler
	@echo "Building MVL compiler..."
	cargo build

build-release: ## Build release binary
	cargo build --release

# === Test ===

MVL ?= ./target/debug/mvl

test: test-corpus test-stdlib test-transpiler test-tree-sitter test-grammar-coverage ## Run all tests (unit + corpus + stdlib + transpiler + tree-sitter grammar + grammar coverage)
	@echo "Running unit tests..."
	cargo test --lib --tests

test-unit: ## Run unit tests only
	cargo test --lib

test-integration: ## Run integration tests
	cargo test --test '*'

test-corpus: ## Validate corpus examples parse and type-check
	@printf "Validating corpus examples..."
	@for f in tests/corpus/**/*.mvl; do \
		if grep -q "corpus:expect-fail" "$$f" 2>/dev/null; then \
			cargo run --quiet -- check "$$f" >/dev/null 2>&1; rc=$$?; \
			if [ $$rc -ne 0 ]; then \
				printf "."; \
			else \
				echo ""; echo "  ERROR: $$f expected violations but checker reported none"; exit 1; \
			fi; \
		else \
			out=$$(cargo run --quiet -- check "$$f" 2>&1); rc=$$?; \
			if [ $$rc -ne 0 ]; then \
				echo ""; echo "  FAIL: $$f"; echo "$$out"; exit 1; \
			fi; \
			printf "."; \
		fi; \
	done
	@echo " ok"

test-stdlib: build ## Verify stdlib runtime correctness: transpile tests/stdlib/ → cargo test
	@echo "Running stdlib correctness tests..."
	$(MVL) test tests/stdlib/

test-transpiler: build ## Run full build-chain tests: .mvl → parse → check → transpile → cargo → binary → verify output
	@echo "Running end-to-end transpiler tests..."
	cargo test --test compile_and_run -- --nocapture
	@echo ""
	@echo "Manual compilation session (using target/debug/mvl):"
	@mvl=./target/debug/mvl; \
	for f in hello_world hello_mvl calculator shapes; do \
		src=$$(find tests/corpus -name "$${f}.mvl" 2>/dev/null | head -1 | tr -d '\n'); \
		echo ""; \
		echo "  --- $$f ---"; \
		if [ -z "$$src" ]; then echo "  SKIP: $${f}.mvl not found in corpus"; continue; fi; \
		$$mvl run "$$src" || exit 1; \
	done

test-llvm: build ## Run Phase B LLVM corpus tests (tests/corpus/02_types/ — always green)
	@echo "Running LLVM backend tests (Phase B corpus)..."
	$(MVL) test tests/corpus/02_types/ --backend=llvm

test-llvm-all: build ## Run all LLVM tests across full corpus (some Phase A+ failures expected)
	@echo "Running LLVM backend tests (full corpus)..."
	$(MVL) test tests/corpus/ --backend=llvm; true

# === Quality ===

lint: ## Lint Rust source with clippy
	cargo clippy -- -D warnings

mvl-lint: build ## Run MVL linter on corpus and examples
	@echo "Running MVL linter on corpus..."
	@failed=0; \
	for f in tests/corpus/**/*.mvl examples/**/*.mvl; do \
		[ -f "$$f" ] || continue; \
		out=$$(cargo run --quiet -- lint "$$f" 2>&1); \
		if [ -n "$$out" ] && echo "$$out" | grep -q "warning\|error"; then \
			echo "$$out"; failed=1; \
		fi; \
	done; \
	if [ $$failed -eq 0 ]; then echo "MVL lint: all clean."; fi

format: ## Format code
	cargo fmt

format-check: ## Check formatting without changing files
	cargo fmt -- --check

# === Assurance ===

coverage: ## Run Rust line coverage via cargo-llvm-cov (cached in target/llvm-cov.json)
	@cargo llvm-cov --json > target/llvm-cov.json 2>/dev/null
	@python3 -c "import json; d=json.load(open('target/llvm-cov.json')); t=d['data'][0]['totals']; l=t['lines']; f=t['functions']; print(f\"Lines: {l['covered']}/{l['count']} ({l['percent']:.1f}%)\"); print(f\"Functions: {f['covered']}/{f['count']} ({f['percent']:.1f}%)\")"

assurance: ## Check ISPE traceability: spec → implementation → tests (verbose with legend)
	@python3 tools/assurance.py --verbose

assurance-summary: ## Assurance dashboard summary only (used by CI)
	@python3 tools/assurance.py

assurance-gate: ## CI gate: fail if below 75% completeness/coverage
	@python3 tools/assurance.py --min 0.75

# === Documentation ===

docs: ## Build documentation site
	bash tools/harvest-specs.sh
	uvx --with mkdocs-material mkdocs build

docs-serve: ## Serve documentation locally (http://localhost:8000)
	bash tools/harvest-specs.sh
	uvx --with mkdocs-material mkdocs serve

# === Clean ===

# === Tree-sitter (editor support) ===
# Grammar is derived from docs/grammar.ebnf — keep in sync manually.

tree-sitter-build: ## Build tree-sitter grammar for Zed/Neovim
	cd etc/tree-sitter-mvl && npm install && npm run build

test-tree-sitter: ## Run tree-sitter corpus tests (grammar derived from docs/grammar.ebnf)
	cd etc/tree-sitter-mvl && npm test

test-grammar-coverage: ## Cross-validate docs/grammar.ebnf against tree-sitter grammar.js
	@python3 tools/check_grammar_coverage.py

install-nvim: ## Install nvim-mvl plugin + compile tree-sitter parser
	etc/nvim-mvl/install.sh


# === Clean ===

clean: ## Clean build artifacts
	cargo clean
	rm -rf build/ site/
