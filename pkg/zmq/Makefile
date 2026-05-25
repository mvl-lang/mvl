# pkg/zmq — ZeroMQ-style messaging patterns in pure MVL
.PHONY: help guard-mvl check lint test prove assurance clean

.DEFAULT_GOAL := help

MVL := ../../target/debug/mvl
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
	$(MVL) check $(DIR)src/

lint: guard-mvl ## Lint source files (missing-totality, style warnings)
	@$(MVL) lint $(DIR)src/

test: guard-mvl ## Run unit tests
	$(MVL) test $(DIR)src/

prove: guard-mvl ## Prove correctness: assurance report with prover verdicts
	@$(MVL) assurance $(DIR)src/ --verbose 2>&1; RC=$$?; \
	echo ""; \
	if [ $$RC -eq 0 ]; then \
		printf "  \033[32m✓  pkg.zmq: assurance PASS\033[0m\n\n"; \
	else \
		printf "  \033[31m✗  pkg.zmq: assurance FAIL\033[0m\n\n"; exit 1; \
	fi

assurance: check lint test prove ## Full pipeline: check + lint + test + prove

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_zmq
