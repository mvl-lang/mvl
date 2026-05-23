# actor_webserver — concurrent HTTP server with actor-per-request pattern
#
# Uses pkg.http for HTTP types, parsing, and response serialization.
# Each accepted TCP connection is handed to a fresh RequestHandler actor that
# owns the socket exclusively (iso capability).
#
# All commands run from REPO_ROOT so pkg.* imports resolve correctly.
.PHONY: help guard-mvl lint check test test-llvm assurance run clean

.DEFAULT_GOAL := help

MVL       := ../../target/debug/mvl
DIR       := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
REPO_ROOT := $(abspath $(DIR)../..)

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

lint: guard-mvl ## Lint main.mvl
	cd $(REPO_ROOT) && $(abspath $(MVL)) lint $(DIR)main.mvl

check: guard-mvl ## Type-check main.mvl
	cd $(REPO_ROOT) && $(abspath $(MVL)) check $(DIR)main.mvl

test: guard-mvl ## Run unit tests (pure routing logic)
	cd $(REPO_ROOT) && $(abspath $(MVL)) test $(DIR)

test-llvm: guard-mvl ## Run smoke tests via LLVM backend (checks // expect: annotations)
	cd $(REPO_ROOT) && $(abspath $(MVL)) test $(DIR) --backend=llvm

assurance: guard-mvl ## Assurance report for main.mvl
	cd $(REPO_ROOT) && $(abspath $(MVL)) assurance $(DIR)main.mvl

run: guard-mvl ## Start the HTTP server on 127.0.0.1:8080 (Ctrl-C to stop)
	cd $(REPO_ROOT) && $(abspath $(MVL)) run $(DIR)main.mvl

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_actor_webserver
