# pkg-zmq -- ZeroMQ-style messaging patterns
.PHONY: help check test test-integration sync-check prove assurance clean

.DEFAULT_GOAL := help

MVL ?= mvl
DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

check: ## Type-check package source files
	$(MVL) check $(DIR)src/

test: ## Run unit tests
	$(MVL) test $(DIR)src/

$(DIR)tests/.mvl/pkg/zmq:
	@mkdir -p $(DIR)tests/.mvl/pkg
	@ln -sfn $(abspath $(DIR)) $(DIR)tests/.mvl/pkg/zmq

test-integration: $(DIR)tests/.mvl/pkg/zmq ## Run ZMTP integration tests (actor-based loopback)
	@printf "  [1/2] Type-checking zmtp_handshake_integration.mvl..."; \
	cd $(DIR)tests && $(MVL) check zmtp_handshake_integration.mvl > /dev/null 2>&1; \
	if [ $$? -ne 0 ]; then printf " FAIL\n"; cd $(DIR)tests && $(MVL) check zmtp_handshake_integration.mvl; exit 1; fi; \
	printf " ok\n"; \
	printf "  [2/2] Transpile + compile + run..."; \
	OUT=$$(cd $(DIR)tests && timeout 60 $(MVL) run zmtp_handshake_integration.mvl 2>&1); RC=$$?; \
	printf "\n"; \
	PASS=$$(echo "$$OUT" | grep -c " ok$$"); \
	FAIL=$$(echo "$$OUT" | grep -c "FAIL"); \
	echo "$$OUT" | grep -E "ok$$|FAIL" | sed 's/^/         /'; \
	if [ $$FAIL -eq 0 ] && [ $$PASS -ge 4 ]; then printf "  pkg.zmq: integration PASS ($$PASS/4)\n"; \
	else printf "  pkg.zmq: integration FAIL ($$PASS passed, $$FAIL failed)\n"; exit 1; fi

sync-check: ## Check test re-declarations match source signatures
	@bash $(DIR)tools/check-sync.sh

prove: ## Prove correctness: assurance report with prover verdicts
	$(MVL) assurance $(DIR)src/ --verbose

assurance: check sync-check prove ## Full pipeline: check + sync-check + prove

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_zmq
