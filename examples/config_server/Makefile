# config_server example — MVL Phase 6 networking + IFC + refinements demonstration.
#
# Demonstrates: ! Net, ! FileRead, ! Log, ! Console effects; Secret[String] IFC labels;
# port/timeout/max_connections refinement types; Tainted → Clean sanitization.
#
# Backend support:
#   transpiler (default) — full demo, extern "rust" bridge linked via bridge.rs
#   LLVM                 — pure tests only (handler_test.mvl); extern stubs return defaults
#
# Run from this directory or from the repo root.
.PHONY: help guard-mvl build check lint test-solver test test-llvm assurance mcdc mutate fuzz run all clean

.DEFAULT_GOAL := help

MVL  := ../../target/debug/mvl
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Validate MVL binary is present (does NOT recompile)
	@$(MVL) --version > /dev/null 2>&1 || { \
		echo ""; \
		echo "  ERROR: MVL compiler not found at: $(MVL)"; \
		echo "  Run 'make build' from the repo root first."; \
		echo ""; \
		exit 1; \
	}

build: guard-mvl ## Compile the example to a native binary (requires bridge.rs)
	$(MVL) build $(DIR)main.mvl

check: guard-mvl ## Type-check all source files individually
	$(MVL) check $(DIR)config.mvl
	$(MVL) check $(DIR)handler.mvl
	$(MVL) check $(DIR)storage.mvl
	$(MVL) check $(DIR)main.mvl

lint: guard-mvl ## Lint all source files
	$(MVL) lint $(DIR)

test-solver: guard-mvl ## Show per-file solver statistics for all source files
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(DIR)config.mvl $(DIR)handler.mvl $(DIR)storage.mvl $(DIR)main.mvl; do \
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

test: guard-mvl ## Run all tests (transpiler backend; extern bridge required)
	$(MVL) test $(DIR)

test-llvm: guard-mvl ## Run pure tests only via LLVM backend (no extern calls)
	$(MVL) test $(DIR)handler_test.mvl --backend=llvm

assurance: guard-mvl ## Assurance report: spec coverage, MC/DC, mutation score (verbose)
	$(MVL) assurance $(DIR) --verbose

mcdc: guard-mvl ## Run MC/DC coverage analysis (ARGS="--masking" to exempt coupled clauses)
	$(MVL) mcdc $(DIR) --verbose $(ARGS)

mutate: guard-mvl ## Run mutation testing
	$(MVL) mutate $(DIR)

fuzz: guard-mvl ## Type-directed fuzzing of handler.mvl Tainted[String] params (requires nightly + cargo-fuzz; ARGS="--time 60s")
	$(MVL) fuzz $(DIR)handler.mvl $(ARGS)

all: check test assurance mcdc ## Run full quality pipeline: check + test + assurance + mcdc

run: guard-mvl ## Execute the demo scenario (loads sample_config.json, 3 simulated requests)
	$(MVL) run $(DIR)main.mvl

clean: ## Remove build artefacts
	rm -rf $(TMPDIR)mvl_build_config_server
