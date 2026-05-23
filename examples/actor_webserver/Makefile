# actor_webserver — concurrent HTTP server with actor-per-request pattern
#
# Uses pkg.http for HTTP types, parsing, and response serialization.
# Each accepted TCP connection is handed to a fresh RequestHandler actor that
# owns the socket exclusively (iso capability).
#
# Package resolution: .mvl/pkg/http symlinks to ../../pkg/http so that
# mvl commands run from this directory can find the package.
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

# Ensure .mvl/pkg/http symlink exists for local package resolution.
.mvl/pkg/http:
	@mkdir -p .mvl/pkg
	@ln -sfn $(REPO_ROOT)/pkg/http .mvl/pkg/http

lint: guard-mvl .mvl/pkg/http ## Lint main.mvl
	$(MVL) lint $(DIR)main.mvl

check: guard-mvl .mvl/pkg/http ## Type-check main.mvl
	$(MVL) check $(DIR)main.mvl

test: guard-mvl .mvl/pkg/http ## Run unit tests (pure routing logic)
	$(MVL) test $(DIR)

test-llvm: guard-mvl .mvl/pkg/http ## Run smoke tests via LLVM backend
	$(MVL) test $(DIR) --backend=llvm

assurance: guard-mvl .mvl/pkg/http ## Assurance report for main.mvl
	$(MVL) assurance $(DIR)main.mvl

run: guard-mvl .mvl/pkg/http ## Start the HTTP server (Ctrl-C to stop)
	$(MVL) run $(DIR)main.mvl

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_actor_webserver .mvl
