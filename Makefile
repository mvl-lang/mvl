# MVL — Minimum Verification Language
.ONESHELL:
SHELL := /bin/bash

.PHONY: help version build build-memory build-release test test-unit test-integration test-corpus test-stdlib test-transpiler test-llvm test-tree-sitter test-grammar-coverage coverage lint mvl-lint format format-check assurance assurance-summary assurance-gate docs docs-serve tree-sitter-build install install-nvim setup doctor clean fuzz-rust fuzz-llvm fuzz-diff

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

build-memory: ## Build mvl_memory cdylib (required by LLVM backend at runtime)
	cargo build -p mvl_memory

build-release: ## Build release binary
	cargo build --release

# === Test ===

MVL ?= ./target/debug/mvl

test: ## Run all test suites and print a one-line PASS/FAIL summary for each
	@pass=0; fail=0; \
	run_suite() { \
		label="$$1"; target="$$2"; \
		out=$$($(MAKE) --no-print-directory "$$target" 2>&1); rc=$$?; \
		if [ $$rc -eq 0 ]; then \
			printf "  \033[32m✓  PASS\033[0m  %s\n" "$$label"; \
			pass=$$((pass + 1)); \
		else \
			printf "  \033[31m✗  FAIL\033[0m  %s\n" "$$label"; \
			printf "%s\n" "$$out" | sed 's/^/         /'; \
			fail=$$((fail + 1)); \
		fi; \
	}; \
	echo ""; \
	run_suite "Unit tests"        test-unit; \
	run_suite "Corpus"            test-corpus; \
	run_suite "Stdlib"            test-stdlib; \
	run_suite "Transpiler"        test-transpiler; \
	run_suite "LLVM backend"      test-llvm; \
	run_suite "Tree-sitter"       test-tree-sitter; \
	run_suite "Grammar coverage"  test-grammar-coverage; \
	echo ""; \
	if [ $$fail -eq 0 ]; then \
		printf "  \033[32m✓  All $$((pass)) suites passed\033[0m\n\n"; \
	else \
		printf "  \033[31m✗  $$fail of $$((pass + fail)) suites failed\033[0m\n\n"; \
		exit 1; \
	fi

test-unit: ## Run unit tests only
	cargo test --lib

test-integration: ## Run integration tests
	cargo test --test '*'

test-corpus: ## Validate corpus examples parse and type-check
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in tests/corpus/**/*.mvl; do \
		short=$${f#tests/corpus/}; \
		if grep -q "corpus:expect-fail" "$$f" 2>/dev/null; then \
			cargo run --quiet -- check "$$f" >/dev/null 2>&1; rc=$$?; \
			if [ $$rc -ne 0 ]; then \
				printf "  $$OK  %s\n" "$$short"; pass=$$((pass + 1)); \
			else \
				printf "  $$FAIL  %s  (expected violations but checker reported none)\n" "$$short"; fail=$$((fail + 1)); \
			fi; \
		else \
			out=$$(cargo run --quiet -- check "$$f" 2>&1); rc=$$?; \
			if [ $$rc -ne 0 ]; then \
				printf "  $$FAIL  %s\n" "$$short"; printf "%s\n" "$$out" | sed 's/^/         /'; fail=$$((fail + 1)); \
			else \
				printf "  $$OK  %s\n" "$$short"; pass=$$((pass + 1)); \
			fi; \
		fi; \
	done; \
	echo ""; \
	if [ $$fail -eq 0 ]; then \
		printf "  \033[32m✓  $$pass passed, 0 failed\033[0m\n\n"; \
	else \
		printf "  \033[31m✗  $$pass passed, $$fail failed\033[0m\n\n"; exit 1; \
	fi

test-stdlib: build ## Verify stdlib runtime correctness: transpile tests/stdlib/ → cargo test
	@echo "Running stdlib correctness tests..."
	$(MVL) test tests/stdlib/

test-transpiler: build ## Run end-to-end transpiler tests: .mvl → parse → check → transpile → cargo → binary → assert output
	cargo test --test compile_and_run

test-llvm: build build-memory ## Run LLVM backend tests across full corpus
	@echo "Running LLVM backend tests (full corpus)..."
	$(MVL) test tests/corpus/ --backend=llvm

# === Quality ===

lint: ## Lint Rust source with clippy
	cargo clippy -- -D warnings

mvl-lint: build ## Run MVL linter on corpus and examples
	@echo "Running MVL linter on corpus..."
	@failed=0; \
	for f in tests/corpus/**/*.mvl examples/**/*.mvl; do \
		[ -f "$$f" ] || continue; \
		case "$$f" in tests/corpus/04_linting/*) continue;; esac; \
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
	@cargo build --manifest-path mvl_memory/Cargo.toml --target-dir target/llvm-cov-target 2>/dev/null
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

# === Fuzzing (long-running — not part of per-PR CI) ===
# Requires: rustup toolchain install nightly && cargo install cargo-fuzz
# See tests/fuzz/README.md for full documentation.

FUZZ_TIMEOUT ?= 5  # default: smoke-test duration; override for real runs

fuzz-rust: ## [Phase 1] Fuzz Rust transpiler pipeline (long-running; set FUZZ_TIMEOUT=86400 for overnight)
	cargo +nightly fuzz run transpile_rust -- -max_total_time=$(FUZZ_TIMEOUT) -timeout=5
	@echo "All clear — no panics found."

fuzz-llvm: ## [Phase 2] Fuzz LLVM codegen pipeline (long-running; set FUZZ_TIMEOUT=86400 for overnight)
	cargo +nightly fuzz run transpile_llvm -- -max_total_time=$(FUZZ_TIMEOUT) -timeout=5
	@echo "All clear — no panics found."

fuzz-diff: ## [Phase 3] Differential fuzzing: Rust vs LLVM backends (subprocess per iter; set FUZZ_TIMEOUT=86400 for overnight)
	@command -v cargo >/dev/null && test -f target/debug/mvl || { echo "Run 'make build' first — fuzz-diff needs the mvl binary."; exit 1; }
	cargo +nightly fuzz run transpile_diff -- -max_total_time=$(FUZZ_TIMEOUT) -timeout=30
	@echo "All clear — no divergences found."

# === Clean ===

clean: ## Clean build artifacts
	cargo clean
	rm -rf build/ site/
