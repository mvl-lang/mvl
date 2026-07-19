# MVL — Maximum Verifiable Language
.ONESHELL:
SHELL := /bin/bash

.PHONY: help version build build-runtime-wasm test test-full test-unit test-rust-integration test-requirements test-error-messages test-fmt-roundtrip test-rust-rust test-rust-llvm test-mvl-llvm test-rust-wasm test-mvl-wasm test-rust-tokio test-runtime-rust test-runtime-llvm test-runtime-wasm test-checker-parity test-checker-parity-update test-solver test-stdlib check-compiler assure-compiler test-mvl test-bootstrap-e2e test-bdd test-grammar-coverage test-examples test-examples-rust test-examples-llvm coverage validate-keywords lint mvl-lint format format-check format-mvl format-mvl-check assurance assurance-gate audit-backend-ast audit-cli-prelude check-adr docs docs-serve install install-runtime setup doctor clean fuzz-rust fuzz-llvm fuzz-diff fuzz-mvl test-fuzz-list mutants mutants-actors

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

setup: ## Install git hooks, init submodules, and verify tooling
	git config core.hooksPath .githooks
	@echo "Git hooks installed from .githooks/"
	@command -v cargo >/dev/null 2>&1 || { echo "cargo not found — install Rust: https://rustup.rs"; exit 1; }
	git submodule update --init --recursive
	cargo install cargo-mutants --locked
	@echo "Ready."
	@echo "Grammar, tree-sitter, and editor extensions live in vendor/mvl-spec/ (submodule of https://github.com/mvl-lang/mvl-spec)"

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
	check wasm-tools    "cargo install wasm-tools  (required for WASM backend)"; \
	check wasmtime      "https://wasmtime.dev/  (required for WASM backend)"; \
	if rustup target list --installed 2>/dev/null | grep -q '^wasm32-wasip1$$'; then \
	  printf "  $$OK wasm32-wasip1 target\n"; \
	else \
	  printf "  $$FAIL wasm32-wasip1 target  (run: rustup target add wasm32-wasip1)\n"; \
	fi; \
	if [ -f target/wasm32-wasip1/debug/mvl_runtime_wasm.wasm ] \
	   || [ -f target/wasm32-wasip1/release/mvl_runtime_wasm.wasm ]; then \
	  printf "  $$OK runtime/wasm/ built  (target/wasm32-wasip1/…/mvl_runtime_wasm.wasm)\n"; \
	else \
	  printf "  $$WARN runtime/wasm/ not built  (run: make build-runtime-wasm)\n"; \
	fi; \
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

# Install paths — versioned toolchain layout under XDG_DATA_HOME (ADR-0009).
# Compiler version drives the toolchain dir; runtime version drives the runtime dir.
# They are tracked independently and may differ (see #1765).
INSTALL_VERSION         := $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
INSTALL_RUNTIME_VERSION := $(shell grep '^version' runtime/rust/Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')

INSTALL_XDG_DATA_HOME   ?= $(HOME)/.local/share
INSTALL_MVL_DATA_DIR    := $(INSTALL_XDG_DATA_HOME)/mvl
INSTALL_TOOLCHAIN_DIR   := $(INSTALL_MVL_DATA_DIR)/toolchains/$(INSTALL_VERSION)
INSTALL_RUNTIME_DIR     := $(INSTALL_MVL_DATA_DIR)/runtime/$(INSTALL_RUNTIME_VERSION)
INSTALL_BIN_DIR         := $(HOME)/.local/bin

install-runtime: build ## Install stdlib + runtime crates from CURRENT $(BUILD) (no mvl binary; for CI matrix)
	@echo "Installing runtime v$(INSTALL_RUNTIME_VERSION) + stdlib from $(BUILD) artifacts ..."
	@mkdir -p $(INSTALL_TOOLCHAIN_DIR)/std
	@mkdir -p $(INSTALL_RUNTIME_DIR)/rust $(INSTALL_RUNTIME_DIR)/rust-tokio $(INSTALL_RUNTIME_DIR)/llvm $(INSTALL_RUNTIME_DIR)/wasm
	rsync -a --delete std/ $(INSTALL_TOOLCHAIN_DIR)/std/
	@echo "$(INSTALL_VERSION)" > $(INSTALL_TOOLCHAIN_DIR)/std/.version
	rsync -a --delete runtime/rust/       $(INSTALL_RUNTIME_DIR)/rust/
	rsync -a --delete runtime/rust-tokio/ $(INSTALL_RUNTIME_DIR)/rust-tokio/
	@cp target/$(BUILD)/libmvl_runtime_llvm.dylib $(INSTALL_RUNTIME_DIR)/llvm/ 2>/dev/null || true
	@cp target/$(BUILD)/libmvl_runtime_llvm.so    $(INSTALL_RUNTIME_DIR)/llvm/ 2>/dev/null || true
	@cp target/wasm32-wasip1/debug/mvl_runtime_wasm.wasm   $(INSTALL_RUNTIME_DIR)/wasm/ 2>/dev/null || true
	@cp target/wasm32-wasip1/release/mvl_runtime_wasm.wasm $(INSTALL_RUNTIME_DIR)/wasm/ 2>/dev/null || true

install: ## Install all artifacts (mvl, stdlib, rust/llvm/wasm runtimes) from local source
	@$(MAKE) build BUILD=release
	@$(MAKE) build-runtime-wasm
	@echo ""
	@echo "Installing mvl $(INSTALL_VERSION) to $(INSTALL_TOOLCHAIN_DIR) ..."
	@mkdir -p $(INSTALL_TOOLCHAIN_DIR)/bin $(INSTALL_TOOLCHAIN_DIR)/std $(INSTALL_BIN_DIR)
	@mkdir -p $(INSTALL_RUNTIME_DIR)/rust $(INSTALL_RUNTIME_DIR)/rust-tokio $(INSTALL_RUNTIME_DIR)/llvm $(INSTALL_RUNTIME_DIR)/wasm
	# 1. mvl binary + ~/.local/bin symlink
	cp target/release/mvl $(INSTALL_TOOLCHAIN_DIR)/bin/mvl
	chmod +x $(INSTALL_TOOLCHAIN_DIR)/bin/mvl
	ln -sfn $(INSTALL_TOOLCHAIN_DIR)/bin/mvl $(INSTALL_BIN_DIR)/mvl
	# 1b. mvlr driver script + ~/.local/bin symlink (#1823)
	cp tools/mvlr $(INSTALL_TOOLCHAIN_DIR)/bin/mvlr
	chmod +x $(INSTALL_TOOLCHAIN_DIR)/bin/mvlr
	ln -sfn $(INSTALL_TOOLCHAIN_DIR)/bin/mvlr $(INSTALL_BIN_DIR)/mvlr
	# 2. stdlib source (.mvl files)
	rsync -a --delete std/ $(INSTALL_TOOLCHAIN_DIR)/std/
	@echo "$(INSTALL_VERSION)" > $(INSTALL_TOOLCHAIN_DIR)/std/.version
	# 3. Rust runtime crate source (default + tokio target)
	rsync -a --delete runtime/rust/       $(INSTALL_RUNTIME_DIR)/rust/
	rsync -a --delete runtime/rust-tokio/ $(INSTALL_RUNTIME_DIR)/rust-tokio/
	# 4. LLVM runtime cdylib — installed in runtime/{ver}/llvm/ (ADR-0009, #1765).
	#    find_mvl_runtime_llvm_lib() resolves current_exe() symlinks and searches
	#    this XDG path first, so no ~/.local/bin/ symlink hack is needed.
	@cp target/release/libmvl_runtime_llvm.dylib $(INSTALL_RUNTIME_DIR)/llvm/ 2>/dev/null || true
	@cp target/release/libmvl_runtime_llvm.so    $(INSTALL_RUNTIME_DIR)/llvm/ 2>/dev/null || true
	# 5. WASM runtime module — target/wasm32-wasip1/{debug,release}/mvl_runtime_wasm.wasm.
	#    Emitted user modules load it via `wasmtime --preload runtime=<path>`; mvl's
	#    `run --backend=wasm` resolves this XDG path via mvlr.
	@cp target/wasm32-wasip1/debug/mvl_runtime_wasm.wasm   $(INSTALL_RUNTIME_DIR)/wasm/ 2>/dev/null || true
	@cp target/wasm32-wasip1/release/mvl_runtime_wasm.wasm $(INSTALL_RUNTIME_DIR)/wasm/ 2>/dev/null || true
	@echo ""
	@echo "Installed:"
	@echo "  binary:       $(INSTALL_BIN_DIR)/mvl -> $(INSTALL_TOOLCHAIN_DIR)/bin/mvl"
	@echo "  driver:       $(INSTALL_BIN_DIR)/mvlr -> $(INSTALL_TOOLCHAIN_DIR)/bin/mvlr"
	@echo "  stdlib:       $(INSTALL_TOOLCHAIN_DIR)/std/"
	@echo "  rust runtime: $(INSTALL_RUNTIME_DIR)/rust/ (v$(INSTALL_RUNTIME_VERSION))"
	@echo "  rust-tokio:   $(INSTALL_RUNTIME_DIR)/rust-tokio/"
	@echo "  llvm runtime: $(INSTALL_RUNTIME_DIR)/llvm/ (v$(INSTALL_RUNTIME_VERSION))"
	@echo "  wasm runtime: $(INSTALL_RUNTIME_DIR)/wasm/ (v$(INSTALL_RUNTIME_VERSION))"

# === Build ===

# Prevent the mvl binary from re-execing to the installed pinned toolchain.
# Without this, `make test` would silently run the installed release binary
# instead of the freshly-built debug binary, making local test runs useless.
export MVL_NO_REEXEC := 1

# BUILD=debug (default) or BUILD=release
BUILD              ?= debug
BUILD_CARGO_FLAGS  := $(if $(filter release,$(BUILD)),--release)

build: ## Build the MVL compiler + LLVM runtime (BUILD=debug|release, default debug)
	@echo "Building MVL compiler + LLVM runtime ($(BUILD)) ..."
	cargo build $(BUILD_CARGO_FLAGS)
	cargo build -p mvl_runtime_llvm $(BUILD_CARGO_FLAGS)

# === Test ===

MVL ?= ./target/debug/mvl
# mvlr — matrix run driver. Prefer the in-tree copy when it exists so a
# dev checkout always runs the mvlr matching this source (the emitter
# under test needs the mvlr that knows how to drive it — the installed
# mvlr may be older and reject unsupported combos like rust/wasm).
# Falls back to the installed binary otherwise, and finally errors out.
MVLR ?= $(shell test -x tools/mvlr && echo tools/mvlr || command -v mvlr 2>/dev/null)

# Suite list for `make test` (fast pre-PR gate) and `make test-full` (full pre-merge gate).
# Format: "label|target" — keep alignment by padding the label.
#
# `test` covers parse/typecheck/lint correctness + stdlib runtime (~10–15 s) — the inner
# loop you want to fail fast on every commit. Codegen, parity, MVL compiler, backends,
# and examples live in `test-full` and run in CI on push-to-main.
TEST_FAST_SUITES := \
	"Unit tests        |test-unit" \
	"Type checker      |test-type-checker" \
	"Requirements      |test-requirements" \
	"Error messages    |test-error-messages" \
	"Fmt roundtrip     |test-fmt-roundtrip" \
	"Backend rust/rust |test-rust-rust" \
	"Solver            |test-solver" \
	"Grammar coverage  |test-grammar-coverage" \
	"Stdlib            |test-stdlib"

TEST_FULL_EXTRA_SUITES := \
	"Checker parity    |test-checker-parity" \
	"MVL compiler      |test-mvl" \
	"BDD               |test-bdd" \
	"Backend rust/llvm |test-rust-llvm" \
	"Backend mvl/llvm  |test-mvl-llvm" \
	"Backend rust/wasm |test-rust-wasm" \
	"Examples (Rust)   |test-examples-rust" \
	"Examples (LLVM)   |test-examples-llvm"

# $(call run_test_suites,SUITES) — accepts a $(...)-expanded suite list and
# emits a per-suite PASS/FAIL summary, exiting non-zero if any suite failed.
define run_test_suites
	@pass=0; fail=0; skip=0; \
	run_suite() { \
		label="$$1"; target="$$2"; \
		out=$$($(MAKE) --no-print-directory "$$target" 2>&1); rc=$$?; \
		if [ $$rc -eq 0 ]; then \
			if echo "$$out" | grep -q "SKIP:"; then \
				reason=$$(echo "$$out" | grep -m1 "SKIP:" | sed 's/.*SKIP: //'); \
				printf "  %-20s  \033[33m~  SKIP\033[0m  %s\n" "$$label" "$$reason"; \
				skip=$$((skip + 1)); \
			else \
				printf "  %-20s  \033[32m✓  PASS\033[0m\n" "$$label"; \
				pass=$$((pass + 1)); \
			fi; \
		else \
			printf "  %-20s  \033[31m✗  FAIL\033[0m\n" "$$label"; \
			printf "%s\n" "$$out" | sed 's/^/         /'; \
			fail=$$((fail + 1)); \
		fi; \
	}; \
	echo ""; \
	for entry in $(1); do \
		label=$${entry%%|*}; target=$${entry##*|}; \
		run_suite "$$label" "$$target"; \
	done; \
	echo ""; \
	if [ $$fail -eq 0 ]; then \
		printf "  \033[32m✓  %d passed, %d skipped\033[0m\n\n" "$$pass" "$$skip"; \
	else \
		printf "  \033[31m✗  %d of %d suites failed (%d skipped)\033[0m\n\n" "$$fail" "$$((pass + fail))" "$$skip"; \
		exit 1; \
	fi
endef

test: build ## Fast pre-PR gate: unit, type checker, rust/rust backend, solver, grammar, stdlib
	$(call run_test_suites,$(TEST_FAST_SUITES))

test-full: build ## Full pre-merge gate: everything in `test` plus codegen, parity, MVL compiler, BDD, backends, examples (~10–20 min)
	$(call run_test_suites,$(TEST_FAST_SUITES) $(TEST_FULL_EXTRA_SUITES))

test-unit: ## Run unit tests only
	cargo test --lib

test-type-checker: ## Run type checker integration tests (IFC, effects, labels, format)
	cargo test --test type_checker

test-rust-integration: build ## Run integration test binaries not covered by any other suite. Excluded: type_checker, requirements, error_messages, fmt_roundtrip (fast gate), checker_parity, compile_and_run (full extra suites).
	cargo test \
		--test assurance \
		--test linter_integration \
		--test manifest_rationale \
		--test meta_commands \
		--test module_resolver \
		--test parser \
		--test solver_corpus \
		--test stdlib \
		--test toolchain \
		--test transpiler \
		--test tools
	@bash tests/integration/compile_and_run/args.sh

test-requirements: ## Run requirement verdict tests — one Proven + one Failed per requirement (1–11)
	cargo test --test requirements -- --test-threads=1

test-error-messages: ## Run error message tests — assert exact diagnostic output for each CheckError variant
	cargo test --test error_messages

test-fmt-roundtrip: ## Run fmt roundtrip tests — verify check(fmt(src)) == check(src) and idempotency
	cargo test --test fmt_roundtrip

test-checker-parity: ## Verify Rust checker verdict over corpus matches baseline (self-hosting #1117)
	@cargo test --test checker_parity --quiet 2>&1 | tail -20

test-checker-parity-update: ## Regenerate checker parity baseline (only when corpus verdicts change intentionally)
	@MVL_UPDATE_PARITY_BASELINE=1 cargo test --test checker_parity --quiet 2>&1 | tail -5

test-solver: build ## Run solver layer programs — real MVL programs of progressing complexity
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in tests/solver/**/*.mvl; do \
		short=$${f#tests/solver/}; \
		if grep -q "solver:expect-fail" "$$f" 2>/dev/null; then \
			$(MVL) check "$$f" >/dev/null 2>&1; rc=$$?; \
			if [ $$rc -ne 0 ]; then \
				printf "  $$OK  %s  (violations detected)\n" "$$short"; pass=$$((pass + 1)); \
			else \
				printf "  $$FAIL  %s  (expected violations but checker reported none)\n" "$$short"; fail=$$((fail + 1)); \
			fi; \
		else \
			out=$$($(MVL) check "$$f" 2>&1); rc=$$?; \
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
	@# Bundle all 38 _test.mvl files into ONE test crate via `mvl test <dir>` —
	@# one transpile pass, one cargo build, one cargo test.  The prior per-file
	@# loop paid a ~2-3s cargo build for each file (~1–2 min total); the bundled
	@# form completes in ~5 s, cache-warm.  Per-file failures still surface via
	@# rustc file:line references pointing back at the offending stdlib test.
	$(MVL) test tests/stdlib/

check-compiler: build ## Verify self-hosted compiler with mvl check + lint (all 4 source files)
	$(MVL) check compiler/
	$(MVL) lint compiler/


assure-compiler: build ## Assurance report for the self-hosted compiler (verbose)
	$(MVL) assurance compiler/ --verbose

test-mvl: build ## Run MVL-in-MVL tests for the self-hosted compiler (compiler/*_test.mvl)
	$(MVL) test compiler/

test-bootstrap-e2e: build ## Tracer bullet: hello_world.mvl → MVL LLVM emitter → llc → cc → run (#1746)
	@printf "  Running hello_world.mvl through self-hosted LLVM emitter...\n"; \
	GOT=$$($(MVLR) --mvl=$(MVL) --compiler=mvl --backend=llvm examples/programs/hello_world.mvl 2>/dev/null); \
	if [ "$$GOT" = "Hello, world!" ]; then \
	  printf "  \033[32m✓\033[0m  hello_world: Hello, world!\n"; \
	else \
	  printf "  \033[31m✗\033[0m  hello_world: expected 'Hello, world!' got '$$GOT'\n"; \
	  exit 1; \
	fi

# Spike tests are INTENTIONALLY excluded from the main `test` target and from CI.
# They explore speculative ideas (issue #187: parser-in-MVL) and require manual invocation.
# See tests/spikes/001-parser/Makefile for per-experiment targets.
test-spikes: build ## Run spike 001-parser tests manually (NOT part of CI — see #683)
	$(MVL) test tests/spikes/001-parser/

test-bdd: build ## Run BDD corpus scenarios with Gherkin report (mvl test --bdd)
	$(MVL) test tests/bdd/ --bdd

# ── New corpus matrix (#1823) ─────────────────────────────────────────────────
# Files are *_test.mvl with `test fn` blocks; a passing return = pass, a
# panic (from assert/assert_eq/assert_ne) = fail. No --expect strings.
# `mvl test <dir>` bundles every _test.mvl file into ONE cargo test crate:
# one transpile pass, one cargo build, one cargo test — same shape as
# test-stdlib. Same corpus runs through every backend; rust/rust is the
# reference. rust/llvm and rust/rust-tokio are fully active; mvl/llvm is
# a tracer bullet; mvl/wasm is a stub (#1828).

# Naming: test-<compiler>-<backend>
#   rust/rust        — Rust compiler → Rust transpiler → cargo test  (active, full corpus)
#   rust/llvm        — Rust compiler → LLVM text emitter → lli       (active, full corpus)
#   mvl/llvm         — MVL self-hosted compiler → LLVM               (tracer bullet, #1828)
#   rust/wasm        — Rust compiler → WAT emitter → wasmtime        (curated spike)
#   mvl/wasm         — MVL self-hosted → WAT                         (stub, #1828)
#   rust/rust-tokio  — Rust compiler → Rust + tokio runtime          (active, 12_actors/ only)

test-rust-rust: build ## rust/rust — new corpus through Rust transpiler (batched, via mvlr)
	$(MVLR) --mvl=$(MVL) --compiler=rust --backend=rust tests/corpus/

test-rust-llvm: build ## rust/llvm — new corpus through LLVM text emitter (via mvlr, see #1828)
	$(MVLR) --mvl=$(MVL) --compiler=rust --backend=llvm tests/corpus/

test-mvl-llvm: build ## mvl/llvm — MVL self-hosted → LLVM (tracer bullet, via mvlr, broader corpus in #1828)
	$(MVLR) --mvl=$(MVL) --compiler=mvl --backend=llvm examples/programs/hello_world.mvl

test-rust-tokio: build ## rust/rust-tokio — actor subset through tokio runtime (tests/corpus/12_actors/)
	$(MVLR) --mvl=$(MVL) --compiler=rust --backend=rust-tokio tests/corpus/12_actors/

test-mvl-wasm: build ## mvl/wasm — MVL self-hosted → WAT (stub, tracked in #1828)
	@printf "  \033[33m~  SKIP: test-mvl-wasm not yet wired\033[0m\n"
	@echo "    Blocker: self-hosted compiler doesn't have a WASM backend yet. See #1828."

test-runtime-rust: ## Unit-test runtime/rust/ crate natively (peer of test-runtime-wasm)
	cargo test -p mvl_runtime_rust

test-runtime-llvm: ## Unit-test runtime/llvm/ crate natively (peer of test-runtime-wasm)
	cargo test -p mvl_runtime_llvm

# WASM cases the backend actually handles — scoped to what runs end-to-end
# without a `runtime/wasm/` crate (Phase 2 of epic #1817). Everything else
# in `tests/corpus/` needs collections, MvlString ops, tagged-union enum
# payloads, closures, or generics-mono — all of which land in later phases.
# Grow this list as the emitter's coverage grows.
WASM_CORPUS := \
	tests/corpus/00_smoke \
	tests/corpus/01_expressions \
	tests/corpus/02_control_flow \
	tests/corpus/03_functions/basic_test.mvl \
	tests/corpus/04_types/enum_test.mvl \
	tests/corpus/05_collections/list_basics_test.mvl \
	tests/corpus/05_collections/list_iter_test.mvl \
	tests/corpus/05_collections/list_get_test.mvl \
	tests/corpus/05_collections/set_test.mvl \
	tests/corpus/05_collections/map_test.mvl \
	tests/corpus/13_stdlib/string_test.mvl

test-rust-wasm: build build-runtime-wasm ## rust/wasm — WASM-supported corpus subset (via runtime/wasm/ preload)
	@command -v wasm-tools > /dev/null 2>&1 || { \
	  printf "  \033[31m✗  wasm-tools not installed — 'cargo install wasm-tools'\033[0m\n"; exit 1; }
	@command -v wasmtime > /dev/null 2>&1 || { \
	  printf "  \033[31m✗  wasmtime not installed — see https://wasmtime.dev/\033[0m\n"; exit 1; }
	MVL_RUNTIME_WASM=$(WASM_RUNTIME_PATH) $(MVLR) --mvl=$(MVL) --compiler=rust --backend=wasm $(WASM_CORPUS)

# runtime/wasm/ — Rust crate compiled to wasm32-wasip1 (#1819). Loaded by
# wasmtime via --preload runtime=<path>. The emitter conditionally emits
# `(import "runtime" ...)` declarations for programs that need it.
WASM_RUNTIME_PATH := $(CURDIR)/target/wasm32-wasip1/debug/mvl_runtime_wasm.wasm

build-runtime-wasm: ## Build runtime/wasm/ crate → wasm32-wasip1 target
	@rustup target list --installed | grep -q wasm32-wasip1 || { \
	  echo "installing wasm32-wasip1 target..."; \
	  rustup target add wasm32-wasip1; }
	cargo build -p mvl_runtime_wasm --target wasm32-wasip1 $(BUILD_CARGO_FLAGS)

test-runtime-wasm: ## Unit-test runtime/wasm/ under wasmtime (wasm32-wasip1 target)
	@rustup target list --installed | grep -q wasm32-wasip1 || { \
	  echo "installing wasm32-wasip1 target..."; \
	  rustup target add wasm32-wasip1; }
	@command -v wasmtime > /dev/null 2>&1 || { \
	  printf "  \033[31m✗  wasmtime not installed — see https://wasmtime.dev/\033[0m\n"; exit 1; }
	cargo test --target wasm32-wasip1 -p mvl_runtime_wasm

test-examples: build ## Run `make test` for every example subdirectory
	@examples/test-all.sh

test-examples-rust: build ## Run Rust transpiler smoke build for every example subdirectory
	@examples/test-all.sh --smoke

test-examples-llvm: build ## Run LLVM backend tests for every example subdirectory
	@examples/test-all.sh --llvm

# === Quality ===

validate-keywords: ## Cross-check keyword lists across mvl-spec EBNF, tree-sitter, compiler/lexer.mvl, and Rust lexer (#706)
	python3 tools/validate_keywords.py

test-grammar-coverage: validate-keywords ## Cross-validate mvl-spec EBNF against the tree-sitter grammar.js
	@python3 tools/check_grammar_coverage.py

lint: ## Lint Rust source with clippy
	cargo clippy -- -D warnings

mvl-lint: build ## Run MVL linter on corpus and examples
	@echo "Running MVL linter on corpus..."
	@failed=0; \
	for f in tests/corpus/**/*.mvl examples/**/*.mvl; do \
		[ -f "$$f" ] || continue; \
		out=$$($(MVL) lint "$$f" 2>&1); \
		if [ -n "$$out" ] && echo "$$out" | grep -q "warning\|error"; then \
			echo "$$out"; failed=1; \
		fi; \
	done; \
	if [ $$failed -eq 0 ]; then echo "MVL lint: all clean."; fi

format: ## Format code
	cargo fmt

format-check: ## Check formatting without changing files
	cargo fmt -- --check

format-mvl: build ## Format all .mvl files in tests/ and std/ in place
	cargo run --quiet -- fmt tests/
	cargo run --quiet -- fmt std/

format-mvl-check: build ## Check that all .mvl files are formatted (CI gate)
	cargo run --quiet -- fmt tests/ --check
	cargo run --quiet -- fmt std/ --check

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
PANIC_BUDGET_PROD := 30
PANIC_BUDGET_TEST := 100
audit-panics: ## Count unreachable!/panic! in src/mvl — split PROD vs TEST, fail if either over budget (#1549)
	@python3 tools/audit_panics.py \
	    --prod-budget $(PANIC_BUDGET_PROD) \
	    --test-budget $(PANIC_BUDGET_TEST)

audit-backend-ast: ## Guard against new parser::ast imports in backends — target 0 (#1594, ADR-0050)
	@python3 tools/audit_backend_ast.py

audit-cli-prelude: ## Guard against direct loader calls in CLI — target 0 (#1803, ADR-0050 extension)
	@python3 tools/audit_cli_prelude.py

audit-test-shadows: ## Guard against test-file shadow declarations — target 0 (pattern 006)
	@python3 tools/audit_test_shadows.py

check-adr: ## Check ADR structure (required sections, no duplicate numbers)
	@python3 tools/check_adr.py --verbose

# === Documentation ===

docs: ## Build documentation site
	bash tools/harvest-specs.sh
	uvx --with mkdocs-material mkdocs build

docs-serve: ## Serve documentation locally (http://localhost:8000)
	bash tools/harvest-specs.sh
	uvx --with mkdocs-material mkdocs serve

# === Grammar / editor tooling ===
# Grammar (EBNF), tree-sitter parser, and editor extensions live in
#   https://github.com/mvl-lang/mvl-spec
# vendored here as a submodule at vendor/mvl-spec/.  See that repo's
# tools/ and editors/ trees for tree-sitter builds and editor installs.
# `make test-grammar-coverage` cross-validates the EBNF against the
# tree-sitter grammar via the pinned submodule.

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
