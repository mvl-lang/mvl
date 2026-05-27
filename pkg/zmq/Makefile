# pkg/zmq — ZeroMQ-style messaging patterns in pure MVL
.PHONY: help guard-mvl check lint test test-integration prove assurance clean

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

# Ensure .mvl/pkg/zmq symlink exists for integration tests.
$(DIR)tests/.mvl/pkg/zmq:
	@mkdir -p $(DIR)tests/.mvl/pkg
	@ln -sfn $(abspath $(DIR)) $(DIR)tests/.mvl/pkg/zmq

test-integration: guard-mvl $(DIR)tests/.mvl/pkg/zmq ## Run ZMTP integration tests (actor-based loopback)
	@echo "Running ZMTP integration tests..."
	@cd $(DIR)tests && $(abspath $(MVL)) check zmtp_handshake_integration.mvl > /dev/null 2>&1 || { \
		echo "  ERROR: type-check failed"; \
		cd $(DIR)tests && $(abspath $(MVL)) check zmtp_handshake_integration.mvl; \
		exit 1; \
	}
	@OUT=$$(cd $(DIR)tests && timeout 30 $(abspath $(MVL)) run zmtp_handshake_integration.mvl 2>&1); RC=$$?; \
	PASS=$$(echo "$$OUT" | grep -c " ok$$"); \
	FAIL=$$(echo "$$OUT" | grep -c "FAIL"); \
	echo "$$OUT" | grep -E "ok$$|FAIL" | sed 's/^/  /'; \
	echo ""; \
	if [ $$FAIL -eq 0 ] && [ $$PASS -ge 4 ]; then \
		printf "  \033[32m✓  pkg.zmq: integration PASS ($$PASS/4)\033[0m\n\n"; \
	else \
		printf "  \033[31m✗  pkg.zmq: integration FAIL ($$PASS passed, $$FAIL failed)\033[0m\n\n"; exit 1; \
	fi

prove: guard-mvl ## Prove correctness: assurance report with prover verdicts
	@$(MVL) assurance $(DIR)src/ --verbose 2>&1; RC=$$?; \
	echo ""; \
	if [ $$RC -eq 0 ]; then \
		printf "  \033[32m✓  pkg.zmq: assurance PASS\033[0m\n\n"; \
	else \
		printf "  \033[31m✗  pkg.zmq: assurance FAIL\033[0m\n\n"; exit 1; \
	fi

coverage: guard-mvl ## Run tests with behavioral branch coverage report
	@$(MVL) test $(DIR)src/ --coverage 2>&1; RC=$$?; \
	echo ""; \
	if [ $$RC -eq 0 ]; then \
		printf "  \033[32m✓  pkg.zmq: coverage PASS\033[0m\n\n"; \
	else \
		printf "  \033[31m✗  pkg.zmq: coverage FAIL\033[0m\n\n"; exit 1; \
	fi

assurance: check lint prove coverage ## Full pipeline: check + lint + prove + coverage (coverage runs tests)

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_zmq
