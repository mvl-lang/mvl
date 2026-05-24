# http_client — HTTP client loopback example
#
# Starts a local HTTP server and makes client requests against it.
# Demonstrates pkg.http.client API with IFC-aware responses.
#
# Package resolution: .mvl/pkg/http symlinks to ../../pkg/http so that
# mvl commands run from this directory can find the package.
.PHONY: help guard-mvl lint check test-solver test test-llvm smoke run clean

.DEFAULT_GOAL := help

MVL       := ../../target/debug/mvl
DIR       := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
REPO_ROOT := $(abspath $(DIR)../..)

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Ensure MVL binary is built and up-to-date
	@cargo build --manifest-path $(DIR)../../Cargo.toml --quiet

# Ensure .mvl/pkg/http symlink exists for local package resolution.
.mvl/pkg/http:
	@mkdir -p .mvl/pkg
	@ln -sfn $(REPO_ROOT)/pkg/http .mvl/pkg/http

lint: guard-mvl .mvl/pkg/http ## Lint main.mvl
	$(MVL) lint $(DIR)main.mvl

check: guard-mvl .mvl/pkg/http ## Type-check main.mvl
	$(MVL) check $(DIR)main.mvl

test-solver: guard-mvl .mvl/pkg/http ## Show per-file solver statistics
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(DIR)main.mvl; do \
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

test: guard-mvl .mvl/pkg/http ## Run unit tests (example files only, not symlinked pkg)
	$(MVL) test $(DIR)main_test.mvl

test-llvm: guard-mvl .mvl/pkg/http ## Run tests via LLVM backend (example files only, not symlinked pkg)
	$(MVL) test $(DIR)main_test.mvl --backend=llvm

smoke: guard-mvl .mvl/pkg/http ## Smoke-test: build binary (proves compilation)
	$(MVL) build $(DIR)main.mvl

run: guard-mvl .mvl/pkg/http ## Run the HTTP client example
	$(MVL) run $(DIR)main.mvl

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_http_client .mvl
