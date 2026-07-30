# actor_pingpong — basic actor-to-actor communication
#
# Demonstrates Phase 8 actor model: actor keyword, pub fn behaviors,
# val/tag capabilities, cross-module actor construction.
#
# The actors live in pingpong.mvl (a sibling module), not main.mvl — main.mvl
# is the CLI entry point; pingpong_test.mvl imports the actors directly
# instead of redeclaring them (see .openspec/patterns/006).
#
# Run from this directory or from the repo root.
.PHONY: help guard-mvl lint check prove test-solver test test-llvm test-wasm assurance smoke run clean

.DEFAULT_GOAL := help

MVL  := ../../target/debug/mvl
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
MVLR := ../../tools/mvlr
WASM_RUNTIME := ../../target/wasm32-wasip1/debug/mvl_runtime_wasm.wasm

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Validate MVL binary is present and working
	@$(MVL) --version > /dev/null 2>&1 || { \
		echo ""; \
		echo "  ERROR: MVL compiler not found at: $(MVL)"; \
		echo "  Run 'make build' from the repo root first."; \
		echo ""; \
		exit 1; \
	}

lint: guard-mvl ## Lint all source files
	$(MVL) lint $(DIR)

check: guard-mvl ## Type-check all source files
	$(MVL) check $(DIR) --verbose

prove: guard-mvl ## Prove per-call-site refinement proof layer breakdown
	$(MVL) prove $(DIR)

test-solver: guard-mvl ## Show per-file solver statistics
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(DIR)pingpong.mvl $(DIR)main.mvl; do \
		name=$$(basename "$$f"); \
		out=$$($(MVL) check "$$f" 2>&1); rc=$$?; \
		if [ $$rc -eq 0 ]; then \
			printf "  $$OK  %s\n" "$$name"; pass=$$((pass + 1)); \
		else \
			printf "  $$FAIL  %s\n" "$$name"; printf "%s\n" "$$out" | sed 's/^/         /'; fail=$$((fail + 1)); \
		fi; \
	done; \
	echo ""; \
	if [ $$fail -eq 0 ]; then \
		printf "  \033[32m✓  $$pass passed, 0 failed\033[0m\n\n"; \
	else \
		printf "  \033[31m✗  $$pass passed, $$fail failed\033[0m\n\n"; exit 1; \
	fi

test: guard-mvl ## Run unit tests (Rust transpiler backend)
	$(MVL) test $(DIR)

test-llvm: guard-mvl ## Run unit tests via LLVM backend
	$(MVL) test $(DIR) --backend=llvm

test-wasm: guard-mvl ## Run unit tests via WASM backend
	MVL_RUNTIME_WASM=$(WASM_RUNTIME) $(MVLR) --mvl=$(MVL) --compiler=rust --backend=wasm $(DIR)

assurance: guard-mvl ## Assurance report for main.mvl
	$(MVL) assurance $(DIR)main.mvl

smoke: guard-mvl ## Smoke-test: build binary (proves compilation without a terminal)
	$(MVL) build $(DIR)main.mvl

run: guard-mvl ## Run the ping-pong exchange
	$(MVL) run $(DIR)main.mvl

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_actor_pingpong
