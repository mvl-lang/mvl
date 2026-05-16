# actor_webserver — HTTP server with actor-per-request pattern (issue #581)
#
# Demonstrates Phase 8 actor model: iso/tag/val capabilities, actor-per-request,
# tag DbPool, structured concurrency.
#
# Run from this directory or from the repo root.
.PHONY: help guard-mvl lint check test test-llvm test-llvm-net assurance run run-net clean

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

lint: guard-mvl ## Lint server.mvl
	$(MVL) lint $(DIR)server.mvl

check: guard-mvl ## Type-check server.mvl
	$(MVL) check $(DIR)server.mvl

test: guard-mvl ## Run unit tests (pure routing logic)
	$(MVL) test $(DIR)

test-llvm: guard-mvl ## Run smoke tests via LLVM backend (checks // expect: annotations)
	$(MVL) test $(DIR) --backend=llvm

assurance: guard-mvl ## Assurance report for server.mvl
	$(MVL) assurance $(DIR)server.mvl

run: guard-mvl ## Start the HTTP server on 127.0.0.1:8080 (Ctrl-C to stop)
	$(MVL) run $(DIR)server.mvl

test-llvm-net: guard-mvl ## Run the self-test TCP server smoke test via LLVM backend
	$(MVL) run $(DIR)net_main.mvl --backend=llvm

run-net: guard-mvl ## Run the self-test TCP server (fires 3 requests and exits)
	$(MVL) run $(DIR)net_main.mvl

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_actor_webserver
