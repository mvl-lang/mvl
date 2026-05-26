# anthropic_chat — Anthropic Claude API example
#
# Demonstrates pkg.anthropic SDK with IFC-aware Secret/Tainted types.
#
# Package resolution: .mvl/pkg/* symlinks to ../../pkg/* so that
# mvl commands run from this directory can find the packages.
.PHONY: help guard-mvl check test-solver test smoke clean

.DEFAULT_GOAL := help

MVL       := ../../target/debug/mvl
DIR       := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
REPO_ROOT := $(abspath $(DIR)../..)

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Ensure MVL binary is present (pre-build with `cargo build` from repo root)
	@$(MVL) --version > /dev/null 2>&1 || { \
		echo ""; \
		echo "  ERROR: MVL binary not found at $(MVL)"; \
		echo "  Run: cargo build from the repo root first."; \
		echo ""; \
		exit 1; \
	}

.mvl/pkg/anthropic:
	@mkdir -p .mvl/pkg
	@ln -sfn $(REPO_ROOT)/pkg/anthropic .mvl/pkg/anthropic

.mvl/pkg/tls:
	@mkdir -p .mvl/pkg
	@ln -sfn $(REPO_ROOT)/pkg/tls .mvl/pkg/tls

check: guard-mvl .mvl/pkg/anthropic .mvl/pkg/tls ## Type-check main.mvl
	$(MVL) check $(DIR)main.mvl

test-solver: guard-mvl .mvl/pkg/anthropic .mvl/pkg/tls ## Show per-file solver statistics
	@OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	out=$$($(MVL) check $(DIR)main.mvl 2>&1); rc=$$?; \
	if [ $$rc -eq 0 ]; then \
		printf "  $$OK  main.mvl\n"; \
	else \
		printf "  $$FAIL  main.mvl\n"; exit 1; \
	fi

test: check ## No unit tests; delegates to type-check

smoke: guard-mvl .mvl/pkg/anthropic .mvl/pkg/tls ## Smoke-test: build + run without API key, verify graceful error
	@OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	out=$$(env -u ANTHROPIC_API_KEY $(MVL) run $(DIR)main.mvl 2>&1); \
	if echo "$$out" | grep -q "error: ANTHROPIC_API_KEY not set"; then \
		printf "  $$OK  anthropic_chat: graceful no-key error\n"; \
	else \
		printf "  $$FAIL  anthropic_chat: unexpected output\n"; \
		echo "$$out"; \
		exit 1; \
	fi

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_*_main .mvl
