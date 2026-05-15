# actor_webserver — HTTP server with actor-per-request pattern (issue #581)
#
# Demonstrates Phase 8 actor model: iso/tag/val capabilities, actor-per-request,
# tag DbPool, structured concurrency.
#
# Run from this directory or from the repo root.
.PHONY: help guard-mvl lint check test test-llvm assurance run clean

.DEFAULT_GOAL := help

MVL  := ../../target/debug/mvl
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

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
	$(MVL) lint $(DIR)main.mvl

check: guard-mvl ## Type-check main.mvl
	$(MVL) check $(DIR)main.mvl --verbose

test: guard-mvl ## Run unit tests (pure routing logic; actor runtime tests pending #695)
	$(MVL) test $(DIR)

test-llvm: guard-mvl ## Run smoke tests via LLVM backend (checks // expect: annotations)
	$(MVL) test $(DIR) --backend=llvm

assurance: guard-mvl ## Assurance report for main.mvl
	$(MVL) assurance $(DIR)main.mvl

run: guard-mvl ## Run the webserver simulation
	$(MVL) run $(DIR)main.mvl

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_actor_webserver
