# pkg/anthropic — Anthropic Claude SDK
.PHONY: help guard-mvl check test assurance clean

.DEFAULT_GOAL := help

ROOT := $(abspath $(dir $(lastword $(MAKEFILE_LIST)))/../..)
MVL := $(ROOT)/target/debug/mvl
DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Validate MVL binary is present
	@$(MVL) --version > /dev/null 2>&1 || { \
		echo ""; \
		echo "  ERROR: MVL compiler not found at: $(MVL)"; \
		echo "  Run 'make build' from the repo root first."; \
		echo ""; \
		exit 1; \
	}

check: guard-mvl ## Type-check package source files
	cd "$(ROOT)" && $(MVL) check $(DIR)src/

test: guard-mvl ## Run unit tests
	cd "$(ROOT)" && $(MVL) test $(DIR)tests/

assurance: guard-mvl ## Full assurance: check + tests + assurance report
	cd "$(ROOT)" && $(MVL) check $(DIR)src/
	cd "$(ROOT)" && $(MVL) test $(DIR)tests/
	cd "$(ROOT)" && $(MVL) assurance $(DIR)src/ --verbose

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_anthropic
