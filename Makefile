# MVL — Minimum Verification Language
.ONESHELL:
SHELL := /bin/bash

.PHONY: help version build build-memory build-llvm-runtime build-release test test-unit test-integration test-requirements test-error-messages test-corpus test-solver test-stdlib check-compiler assure-compiler test-mvl test-bdd test-backend-rust test-backend-llvm test-cross-backend test-tree-sitter test-grammar-coverage test-examples coverage validate-keywords lint mvl-lint format format-check assurance assurance-gate check-adr docs docs-serve tree-sitter-build install install-nvim setup doctor clean fuzz-rust fuzz-llvm fuzz-diff fuzz-mvl test-fuzz-list mutants mutants-actors

.DEFAULT_GOAL := help

help: ## Show this help
	@echo ""
	@awk 'BEGIN {FS = ":.*?## "} \
	  /^# === .* ===$$/  { sub(/^# === /, ""); sub(/ ===$$/, ""); printf "\n\033[33m%s\033[0m\n", $$0 } \
	  /^[a-zA-Z0-9_-]+:.*?## / { printf "  \033[36m%-24s\033[0m %s\n", $$1, $$2 }' \
	  $(MAKEFILE_LIST)
	@echo ""

version: ## Show current project version
	@grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/'

# === Setup ===

setup: ## Install git hooks, verify tooling, and install tree-sitter npm deps
	git config core.hooksPath .githooks
	@echo "Git hooks installed from .githooks/"
	@command -v cargo >/dev/null 2>&1 || { echo "cargo not found — install Rust: https://rustup.rs"; exit 1; }
	@command -v node >/dev/null 2>&1 || { echo "node not found — install Node.js: https://nodejs.org"; exit 1; }
	cd etc/tree-sitter-mvl && npm install
	cargo install cargo-mutants --locked
	@echo "Ready."

doctor: ## Check that all dev tools are available
	@echo "Checking dev tools..."; echo; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; WARN="\033[33m!\033[0m"; \
	check() { command -v "$$1" >/dev/null 2>&1 && printf "  $$OK $$1\n" || printf "  $$FAIL $$1  ($$2)\n"; }; \
	check cargo         "https://rustup.rs"; \
	check rustfmt       "rustup component add rustfmt"; \
	check clippy-driver "rustup component add clippy"; \
	check node          "https://nodejs.org"; \
	check python3       "required for make assurance"; \
	check /opt/homebrew/opt/llvm/bin/lli "brew install llvm  (required for LLVM backend)"; \
	WANT=$$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)"/\1/'); \
	GOT=$$(mvl --version 2>/dev/null | awk '{print $$2}'); \
	if [ -z "$$GOT" ]; then \
	  printf "  $$FAIL mvl not installed  (run: make install)\n"; \
	elif [ "$$GOT" != "$$WANT" ]; then \
	  printf "  $$WARN mvl $$GOT installed but project is $$WANT  (run: make install)\n"; \
	else \
	  printf "  $$OK mvl $$GOT\n"; \
	fi; \
	echo

install: build-release build-llvm-runtime-release ## Install mvl binary + LLVM runtime to ~/.local/bin
	@mkdir -p ~/.local/bin
	cp target/release/mvl ~/.local/bin/mvl
	cp target/release/libmvl_runtime_c.dylib ~/.local/bin/libmvl_runtime_c.dylib 2>/dev/null || true
	cp target/release/libmvl_runtime_c.so    ~/.local/bin/libmvl_runtime_c.so    2>/dev/null || true
	@echo "Installed: ~/.local/bin/mvl"

# === Build ===

build: ## Build the MVL compiler
	@echo "Building MVL compiler..."
	cargo build

build-llvm-runtime: ## Build the LLVM runtime cdylib (mvl_runtime_c at runtime/llvm)
	cargo build -p mvl_runtime_c

build-llvm-runtime-release: ## Build the LLVM runtime cdylib in release mode
	cargo build --release -p mvl_runtime_c

build-release: ## Build release binary
	cargo build --release

# === Test ===

MVL ?= ./target/debug/mvl

test: build build-llvm-runtime ## Run all test suites and print a one-line PASS/FAIL summary for each
	@pass=0; fail=0; \
	run_suite() { \
		label="$$1"; target="$$2"; \
		out=$$($(MAKE) --no-print-directory "$$target" 2>&1); rc=$$?; \
		if [ $$rc -eq 0 ]; then \
			printf "  %-20s  \033[32m✓  PASS\033[0m\n" "$$label"; \
			pass=$$((pass + 1)); \
		else \
			printf "  %-20s  \033[31m✗  FAIL\033[0m\n" "$$label"; \
			printf "%s\n" "$$out" | sed 's/^/         /'; \
			fail=$$((fail + 1)); \
		fi; \
	}; \
	echo ""; \
	run_suite "Unit tests"        test-unit; \
	run_suite "Requirements"      test-requirements; \
	run_suite "Error messages"    test-error-messages; \
	run_suite "Corpus"            test-corpus; \
	run_suite "Solver"            test-solver; \
	run_suite "Stdlib"            test-stdlib; \
	run_suite "BDD"               test-bdd; \
	run_suite "Backend (Rust)"    test-backend-rust; \
	run_suite "LLVM backend"      test-backend-llvm; \
	run_suite "Cross-backend"     test-cross-backend; \
	run_suite "Examples"          test-examples; \
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

test-integration: ## Dev convenience: run all integration test binaries at once (may overlap with named targets in make test)
	cargo test --tests
	@bash tests/integration/compile_and_run/args.sh

test-requirements: ## Run requirement verdict tests — one Proven + one Failed per requirement (1–11)
	cargo test --test requirements -- --test-threads=1

test-error-messages: ## Run error message tests — assert exact diagnostic output for each CheckError variant
	cargo test --test error_messages

test-corpus: ## Validate corpus examples parse and type-check
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	while IFS= read -r f; do \
		short=$${f#tests/corpus/}; \
		[[ "$$f" == *_test.mvl ]] && continue; \
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
	done < <(find tests/corpus -name "*.mvl" | sort); \
	echo ""; \
	if [ $$fail -eq 0 ]; then \
		printf "  \033[32m✓  $$pass passed, 0 failed\033[0m\n\n"; \
	else \
		printf "  \033[31m✗  $$pass passed, $$fail failed\033[0m\n\n"; exit 1; \
	fi

test-solver: build ## Run solver layer programs — real MVL programs of progressing complexity
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in tests/solver/**/*.mvl; do \
		short=$${f#tests/solver/}; \
		if grep -q "solver:expect-fail" "$$f" 2>/dev/null; then \
			cargo run --quiet -- check "$$f" >/dev/null 2>&1; rc=$$?; \
			if [ $$rc -ne 0 ]; then \
				printf "  $$OK  %s  (violations detected)\n" "$$short"; pass=$$((pass + 1)); \
			else \
				printf "  $$FAIL  %s  (expected violations but checker reported none)\n" "$$short"; fail=$$((fail + 1)); \
			fi; \
		else \
			out=$$(cargo run --quiet -- check "$$f" 2>&1); rc=$$?; \
			if [ $$rc -eq 0 ]; then \
				printf "  $$OK  %s\n" "$$short"; pass=$$((pass + 1)); \
			else \
				printf "  $$FAIL  %s\n" "$$short"; printf "%s\n" "$$out" | sed 's/^/         /'; fail=$$((fail + 1)); \
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

check-compiler: build ## Verify self-hosted compiler with mvl check + lint (all 4 source files)
	$(MVL) check compiler/
	$(MVL) lint compiler/

assure-compiler: build ## Assurance report for the self-hosted compiler (verbose)
	$(MVL) assurance compiler/ --verbose

test-mvl: build ## Run MVL-in-MVL tests for the self-hosted compiler (compiler/*_test.mvl)
	$(MVL) test compiler/

# Spike tests are INTENTIONALLY excluded from the main `test` target and from CI.
# They explore speculative ideas (issue #187: parser-in-MVL) and require manual invocation.
# See tests/spikes/001-parser/Makefile for per-experiment targets.
test-spikes: build ## Run spike 001-parser tests manually (NOT part of CI — see #683)
	$(MVL) test tests/spikes/001-parser/

test-bdd: build ## Run BDD corpus scenarios with Gherkin report (mvl test --bdd)
	$(MVL) test tests/corpus/11_bdd/ --bdd

test-backend-rust: build ## Run end-to-end transpiler tests: .mvl → parse → check → transpile → cargo → binary → assert output
	cargo test --test compile_and_run

test-backend-llvm: build build-llvm-runtime ## Run LLVM backend tests across full corpus + intrinsics
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	while IFS= read -r line; do \
		case "$$line" in \
			"  PASS: "*) f="$${line#  PASS: }"; short="$${f#tests/}"; printf "  $$OK  %s\n" "$$short"; pass=$$((pass + 1));; \
			"  FAIL"*) f="$${line##*: }"; short="$${f#tests/}"; printf "  $$FAIL  %s\n" "$$short"; fail=$$((fail + 1));; \
		esac; \
	done < <({ $(MVL) test tests/corpus/ --backend=llvm --verbose; $(MVL) test tests/intrinsics/ --backend=llvm --verbose; $(MVL) test tests/stdlib/ --backend=llvm --verbose; } 2>&1); \
	echo ""; \
	if [ $$fail -eq 0 ]; then \
		printf "  \033[32m✓  $$pass passed, 0 failed\033[0m\n\n"; \
	else \
		printf "  \033[31m✗  $$pass passed, $$fail failed\033[0m\n\n"; exit 1; \
	fi

test-cross-backend: build build-llvm-runtime ## Run Rust integration tests for backend parity (transpiler vs LLVM)
	@echo "Running cross-backend tests (transpiler vs LLVM parity)..."
	cargo test --test cross_backend

test-examples: build build-llvm-runtime ## Run `make test` for every example subdirectory (BACKEND=llvm for LLVM backend)
	@examples/test-all.sh $(if $(filter llvm,$(BACKEND)),--llvm)

# === Quality ===

validate-keywords: ## Cross-check keyword lists across EBNF, tree-sitter, compiler/lexer.mvl, and Rust lexer (#706)
	python3 tools/validate_keywords.py

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
	@cargo llvm-cov --json --ignore-run-fail > target/llvm-cov.json 2>/dev/null
	@python3 -c "import json; d=json.load(open('target/llvm-cov.json')); t=d['data'][0]['totals']; l=t['lines']; f=t['functions']; print(f\"Lines: {l['covered']}/{l['count']} ({l['percent']:.1f}%)\"); print(f\"Functions: {f['covered']}/{f['count']} ({f['percent']:.1f}%)\")"

assurance: ## Assurance dashboard (add VERBOSE=true for full output with legend)
	@python3 tools/assurance.py $(if $(VERBOSE),--verbose)

assurance-gate: ## CI gate: fail if below 75% completeness/coverage
	@python3 tools/assurance.py --min 0.75

# Budget for total unreachable!/panic! calls in src/mvl/ (production + inline tests).
# This count includes test assertion helpers (which are fine) alongside production
# unreachables.  The purpose is to detect new additions: raise the budget only when
# a deliberate new unreachable!/panic! is added with a documented reason (#991).
# Baseline after #990 cleanup: 98.
PANIC_BUDGET := 100
audit-panics: ## Count unreachable!/panic! in src/mvl — fail if over budget (#991)
	@count=$$(grep -rn 'unreachable!\|panic!' src/mvl/ --include='*.rs' \
	    | grep -v '"panic"\|"panic!' \
	    | wc -l | tr -d ' '); \
	echo "unreachable!/panic! count: $$count (budget: $(PANIC_BUDGET))"; \
	if [ "$$count" -gt "$(PANIC_BUDGET)" ]; then \
	    echo "FAIL: count $$count exceeds budget $(PANIC_BUDGET) — see issue #991"; exit 1; \
	fi

check-adr: ## Check ADR structure (required sections, no duplicate numbers)
	@python3 tools/check_adr.py --verbose

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

test-tree-sitter: tree-sitter-build ## Run tree-sitter corpus tests (grammar derived from docs/grammar.ebnf)
	cd etc/tree-sitter-mvl && npm test

test-grammar-coverage: validate-keywords ## Cross-validate docs/grammar.ebnf against tree-sitter grammar.js
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

fuzz-mvl: build ## [Phase 8] Type-directed runtime fuzzing of MVL programs (Tainted[T] params; set FUZZ_TIMEOUT=60 for real runs)
	@command -v cargo +nightly >/dev/null 2>&1 || { echo "error: nightly toolchain required — rustup toolchain install nightly"; exit 1; }
	target/debug/mvl fuzz examples/log_analyzer --target parse_line --time $(FUZZ_TIMEOUT)s

test-fuzz-list: build ## Smoke-test mvl fuzz --list on all examples with Tainted[T] params (no nightly required)
	@echo "Checking fuzz target discovery..."
	@ok=0; fail=0; \
	for dir in examples/log_analyzer examples/task_pipeline examples/config_server; do \
		out=$$(target/debug/mvl fuzz $$dir --list 2>&1); rc=$$?; \
		if [ $$rc -eq 0 ]; then \
			printf "  \033[32m✓\033[0m  $$dir\n"; echo "$$out" | sed 's/^/       /'; ok=$$((ok+1)); \
		else \
			printf "  \033[31m✗\033[0m  $$dir\n"; echo "$$out" | sed 's/^/       /'; fail=$$((fail+1)); \
		fi; \
	done; \
	echo ""; \
	if [ $$fail -eq 0 ]; then \
		printf "  \033[32m✓  $$ok example(s) — fuzz target discovery working\033[0m\n\n"; \
	else \
		printf "  \033[31m✗  $$fail example(s) failed\033[0m\n\n"; exit 1; \
	fi

# === Mutation testing (long-running — not part of per-PR CI) ===
# Scores transpiler emit_*.rs modules; target: ≥80% mutation score.
# Results written to mutants.out/ — see mutants.out/outcomes.json for triage.
# Ref: #206

MUTANTS_TIMEOUT ?= 120  # seconds per mutant; raise for slow machines

mutants: ## Run cargo-mutants on transpiler emit modules (long-running; ~1-2 h)
	cargo mutants \
	  --file 'src/mvl/transpiler/emit_exprs.rs' \
	  --file 'src/mvl/transpiler/emit_stmts.rs' \
	  --file 'src/mvl/transpiler/emit_types.rs' \
	  --timeout $(MUTANTS_TIMEOUT) \
	  --jobs 4 \
	  --cargo-test-arg '--test' \
	  --cargo-test-arg 'transpiler'
	@echo ""
	@echo "Results in mutants.out/  — run 'cat mutants.out/caught.txt' and 'cat mutants.out/missed.txt'"

# Scores actor checker + backend codegen; target: ≥85% mutation score.
# Ref: #703
mutants-actors: ## Run cargo-mutants on actor checker and codegen (long-running; ~1-2 h)
	cargo mutants \
	  --file 'src/mvl/checker/capabilities.rs' \
	  --file 'src/mvl/checker/decls.rs' \
	  --file 'src/mvl/checker/data_race.rs' \
	  --file 'src/mvl/backends/rust/emit_actors.rs' \
	  --file 'src/mvl/backends/llvm/actors.rs' \
	  --timeout $(MUTANTS_TIMEOUT) \
	  --jobs 4 \
	  --cargo-test-arg '--test' \
	  --cargo-test-arg 'type_checker' \
	  --cargo-test-arg '--test' \
	  --cargo-test-arg 'transpiler'
	@echo ""
	@echo "Results in mutants.out/  — run 'cat mutants.out/caught.txt' and 'cat mutants.out/missed.txt'"

# === Clean ===

clean: ## Clean build artifacts (target/, fuzz corpus/artifacts, benchmark reports, site)
	cargo clean
	rm -rf build/ site/
	rm -rf fuzz/corpus/ fuzz/artifacts/
	rm -rf mutants.out/
