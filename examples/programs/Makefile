# examples/programs — MVL showcase programs
# Run from this directory or via `make test-examples` from the repo root.
.ONESHELL:
SHELL := /bin/bash
.PHONY: help check prove lint test-solver test test-rust test-llvm test-wasm assurance smoke run clean

MVL  := ../../target/debug/mvl
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
WASM_RUNTIME := ../../target/wasm32-wasip1/debug/mvl_runtime_wasm.wasm

# Every program here is a `fn main` + `// Expected stdout:`/`// expect:`/
# `// expect-pattern:` annotated file, not a `_test.mvl` test-fn file — `mvl
# test` discovers and runs that annotation style natively per backend (the
# expect-parser is shared across backends: src/mvl/backends/llvm_text/lli.rs),
# so no synthesized driver is needed. Sibling-only files with no `fn main`
# (e.g. sibling_dispatch/point.mvl) find no cases and are silently skipped,
# not counted either way.
#
# Each backend below excludes a small, explicitly-documented set of known
# gaps instead of blocking the whole target on them — mirrors
# WASM_CORPUS_EXCLUDE in the root Makefile. Files run individually (not the
# whole $(DIR) in one `mvl test` call) so an exclude list only skips what's
# actually broken, not everything else too.
ALL_PROGRAMS := $(sort $(shell find $(DIR) -name "*.mvl"))

RUST_EXCLUDE :=

LLVM_EXCLUDE := \
	$(DIR)random_dice/main.mvl

WASM_EXCLUDE := \
	$(DIR)auth_handler.mvl \
	$(DIR)closure_lambdas.mvl \
	$(DIR)core_types_demo.mvl \
	$(DIR)hof_lambdas.mvl \
	$(DIR)list_stubs.mvl \
	$(DIR)random_dice/main.mvl

# password_checker.mvl: intentional negative fixture for Spec 006 Requirement 3
# (Missing Bridge Error) — `mvl build` on it MUST fail with "bridge.rs not
# found", per tests/compile_and_run.rs::missing_bridge_exits_nonzero_with_actionable_error.
# Not a gap to fix; bridge_ok/ and access_control/ cover the working-bridge case.
SMOKE_EXCLUDE := \
	$(DIR)password_checker.mvl

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

check: ## Type-check all example programs
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	while IFS= read -r f; do \
		short=$${f#$(DIR)}; \
		out=$$($(MVL) check "$$f" 2>&1); rc=$$?; \
		if [ $$rc -eq 0 ]; then \
			printf "  $$OK  %s\n" "$$short"; pass=$$((pass + 1)); \
		else \
			printf "  $$FAIL  %s\n" "$$short"; printf "%s\n" "$$out" | sed 's/^/         /'; fail=$$((fail + 1)); \
		fi; \
	done < <(find $(DIR) -name "*.mvl" | sort); \
	echo ""; \
	if [ $$fail -eq 0 ]; then \
		printf "  \033[32m✓  $$pass passed, 0 failed\033[0m\n\n"; \
	else \
		printf "  \033[31m✗  $$pass passed, $$fail failed\033[0m\n\n"; exit 1; \
	fi

prove: ## Prove per-call-site refinement proof layer breakdown
	$(MVL) prove $(DIR)

lint: ## Lint all example programs
	$(MVL) lint $(DIR)

test-solver: check ## Alias for check — per-file solver statistics

test: check ## Alias for check (programs dir has no test functions)

# random_dice/main.mvl: extern "rust" FFI unsupported on --backend=llvm
# (#2049). auth_handler.mvl's former exclusion (struct-payload enum match
# crash, #2075) is fixed — it passes here now.
test-llvm: ## Run all example programs via LLVM backend
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(filter-out $(LLVM_EXCLUDE),$(ALL_PROGRAMS)); do \
		short=$${f#$(DIR)}; \
		out=$$($(MVL) test "$$f" --backend=llvm 2>&1); rc=$$?; \
		if echo "$$out" | grep -q "^No .* test cases found"; then \
			continue; \
		elif [ $$rc -eq 0 ]; then \
			printf "  $$OK  %s\n" "$$short"; pass=$$((pass + 1)); \
		else \
			printf "  $$FAIL  %s\n" "$$short"; printf "%s\n" "$$out" | sed 's/^/         /'; fail=$$((fail + 1)); \
		fi; \
	done; \
	echo ""; \
	if [ $$fail -eq 0 ]; then \
		printf "  \033[32m✓  $$pass passed, 0 failed\033[0m\n\n"; \
	else \
		printf "  \033[31m✗  $$pass passed, $$fail failed\033[0m\n\n"; exit 1; \
	fi

# closure_lambdas.mvl, core_types_demo.mvl, hof_lambdas.mvl, list_stubs.mvl:
# each traps (`unreachable`) at runtime on the WASM backend — separate,
# pre-existing codegen gaps (closures/HOF lambdas unsupported, a couple of
# missing builtins). Worth their own issues.
# auth_handler.mvl: its #2075 match-dispatch crash is fixed, but it still
# fails on WASM for a different, newly-found reason — struct field access
# through a refined-type alias (`type UserId = Int where self > 0`) returns
# a wrong equality result on WASM, breaking find_user's linear scan.
# random_dice/main.mvl: extern "rust" FFI unsupported on --backend=wasm
# (#2049).
test-wasm: ## Run all example programs via WASM backend
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(filter-out $(WASM_EXCLUDE),$(ALL_PROGRAMS)); do \
		short=$${f#$(DIR)}; \
		out=$$(MVL_RUNTIME_WASM=$(WASM_RUNTIME) $(MVL) test "$$f" --backend=wasm 2>&1); rc=$$?; \
		if echo "$$out" | grep -q "^No .* test cases found"; then \
			continue; \
		elif [ $$rc -eq 0 ]; then \
			printf "  $$OK  %s\n" "$$short"; pass=$$((pass + 1)); \
		else \
			printf "  $$FAIL  %s\n" "$$short"; printf "%s\n" "$$out" | sed 's/^/         /'; fail=$$((fail + 1)); \
		fi; \
	done; \
	echo ""; \
	if [ $$fail -eq 0 ]; then \
		printf "  \033[32m✓  $$pass passed, 0 failed\033[0m\n\n"; \
	else \
		printf "  \033[31m✗  $$pass passed, $$fail failed\033[0m\n\n"; exit 1; \
	fi

# --expect runs the same annotation style through the Rust transpiler
# (test::run_expect_tests) — no exclusions currently needed, every program
# passes on this backend.
test-rust: ## Run all example programs via the Rust transpiler backend
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(filter-out $(RUST_EXCLUDE),$(ALL_PROGRAMS)); do \
		short=$${f#$(DIR)}; \
		out=$$($(MVL) test "$$f" --expect 2>&1); rc=$$?; \
		if echo "$$out" | grep -q "^No .* test cases found\|^No \*_test.mvl files found"; then \
			continue; \
		elif [ $$rc -eq 0 ]; then \
			printf "  $$OK  %s\n" "$$short"; pass=$$((pass + 1)); \
		else \
			printf "  $$FAIL  %s\n" "$$short"; printf "%s\n" "$$out" | sed 's/^/         /'; fail=$$((fail + 1)); \
		fi; \
	done; \
	echo ""; \
	if [ $$fail -eq 0 ]; then \
		printf "  \033[32m✓  $$pass passed, 0 failed\033[0m\n\n"; \
	else \
		printf "  \033[31m✗  $$pass passed, $$fail failed\033[0m\n\n"; exit 1; \
	fi

assurance: ## Assurance report for all example programs (verbose)
	$(MVL) assurance $(DIR) --verbose

smoke: ## Smoke-test: build every program (proves compilation without running)
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(filter-out $(SMOKE_EXCLUDE),$(ALL_PROGRAMS)); do \
		short=$${f#$(DIR)}; \
		out=$$($(MVL) build "$$f" 2>&1); rc=$$?; \
		if [ $$rc -eq 0 ]; then \
			printf "  $$OK  %s\n" "$$short"; pass=$$((pass + 1)); \
		else \
			printf "  $$FAIL  %s\n" "$$short"; printf "%s\n" "$$out" | sed 's/^/         /'; fail=$$((fail + 1)); \
		fi; \
	done; \
	echo ""; \
	if [ $$fail -eq 0 ]; then \
		printf "  \033[32m✓  $$pass passed, 0 failed\033[0m\n\n"; \
	else \
		printf "  \033[31m✗  $$pass passed, $$fail failed\033[0m\n\n"; exit 1; \
	fi
