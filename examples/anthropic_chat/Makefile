# anthropic_chat — Anthropic Claude API example
#
# Demonstrates pkg.anthropic SDK with IFC-aware Secret/Tainted types.
#
# Packages are resolved from the XDG cache ($XDG_DATA_HOME/mvl/pkg/).
# Run `mvl install` from the repo root to populate the cache before using this example.
.PHONY: help guard-mvl check test-solver test assurance smoke clean

.DEFAULT_GOAL := help

MVL  := ../../target/debug/mvl
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

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

check: guard-mvl ## Type-check main.mvl
	$(MVL) check $(DIR)main.mvl

test-solver: guard-mvl ## Show per-file solver statistics
	@OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	out=$$($(MVL) check $(DIR)main.mvl 2>&1); rc=$$?; \
	if [ $$rc -eq 0 ]; then \
		printf "  $$OK  main.mvl\n"; \
	else \
		printf "  $$FAIL  main.mvl\n"; exit 1; \
	fi

test: ## Requires external package — skipped in test-all, use `make check` after `mvl install`
	@echo "  SKIP: anthropic_chat requires \`mvl install\` first (external package: pkg-anthropic)"

assurance: guard-mvl ## Assurance report for main.mvl
	$(MVL) assurance $(DIR)main.mvl

smoke: guard-mvl ## Smoke-test: build + run without API key, verify graceful error
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
	rm -rf $(TMPDIR)mvl_build_*_main
