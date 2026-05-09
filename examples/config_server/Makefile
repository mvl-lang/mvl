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
.PHONY: help guard-mvl build check test test-llvm run clean

MVL  := ../../target/debug/mvl
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

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

test: guard-mvl ## Run all tests (transpiler backend; extern bridge required)
	$(MVL) test $(DIR)

test-llvm: guard-mvl ## Run pure tests only via LLVM backend (no extern calls)
	$(MVL) test $(DIR)handler_test.mvl --backend=llvm

run: guard-mvl ## Execute the demo scenario (loads sample_config.json, 3 simulated requests)
	$(MVL) run $(DIR)main.mvl

clean: ## Remove build artefacts
	rm -rf $(TMPDIR)mvl_build_config_server
