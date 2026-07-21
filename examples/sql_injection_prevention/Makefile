# sql_injection_prevention example — QF-Strings + IFC-on-taint demonstration (OWASP)
# Run from this directory or from the repo root.
.PHONY: help guard-mvl check prove lint test-solver test test-llvm coverage assurance mcdc mutate smoke run run-llvm all clean test-owasp

.DEFAULT_GOAL := help

MVL  := ../../target/debug/mvl
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl:
	@$(MVL) --version > /dev/null 2>&1 || { \
		echo ""; \
		echo "  ERROR: MVL compiler not found at: $(MVL)"; \
		echo "  Run 'make build' from the repo root first."; \
		echo ""; \
		exit 1; \
	}

check: guard-mvl ## Type-check all source files
	$(MVL) check $(DIR)

prove: guard-mvl ## Prove per-call-site refinement proof layer breakdown
	$(MVL) prove $(DIR)

lint: guard-mvl ## Lint all source files
	$(MVL) lint $(DIR)

test-solver: guard-mvl ## Show per-file solver statistics for all source files
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(DIR)model.mvl $(DIR)injection.mvl $(DIR)main.mvl; do \
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

test: guard-mvl ## Run unit tests
	$(MVL) test $(DIR)

test-llvm: guard-mvl ## Run unit tests via LLVM backend
	$(MVL) test $(DIR) --backend=llvm

coverage: guard-mvl ## Run unit tests with native behavioral branch coverage report
	$(MVL) test $(DIR) --coverage

mcdc: guard-mvl ## Run MC/DC coverage analysis (use ARGS="--masking" to exempt coupled)
	$(MVL) mcdc $(DIR) --verbose $(ARGS)

mutate: guard-mvl ## Run mutation testing
	$(MVL) mutate $(DIR)

assurance: guard-mvl ## Assurance report: spec coverage, MC/DC, mutation score (verbose)
	$(MVL) assurance $(DIR) --verbose

all: check test assurance ## Full pipeline: check + test + assurance + coverage + MC/DC (with DO-178C masking)
	$(MVL) test $(DIR) --coverage
	$(MVL) mcdc $(DIR) --verbose --masking

test-owasp: guard-mvl ## OWASP A03:2021 assurance envelope: static proof + tests + coverage + MC/DC
	@echo "════════════════════════════════════════════════════════════════"
	@echo "  OWASP A03:2021 assurance envelope — sql_injection_prevention"
	@echo "  Input validation (SQL injection prevention): three independent axes"
	@echo "════════════════════════════════════════════════════════════════"
	@echo ""
	@echo "── (1) Static refinement proof (compile-time, all inputs) ─────"
	@$(MVL) prove $(DIR) 2>&1 | grep -E "Total:" || echo "  FAIL: no prove summary"
	@echo ""
	@echo "── (2) Behavioural unit tests (dynamic, specific inputs) ──────"
	@$(MVL) test $(DIR) 2>&1 | grep -E "test result|All tests" | tail -2
	@echo ""
	@echo "── (3) Branch coverage (decision points reached) ──────────────"
	@$(MVL) test $(DIR) --coverage 2>&1 | grep -E "Total:" | tail -1
	@echo ""
	@echo "── (4) MC/DC coverage (compound decisions, --masking) ─────────"
	@$(MVL) mcdc $(DIR) --masking 2>&1 | grep -E "MC/DC coverage|PASS|FAIL" | tail -3
	@echo ""
	@echo "── Audit anchors (grep -n MCDC- injection.mvl) ────────────────"
	@grep -n "MCDC-SQLINJ-" $(DIR)injection.mvl | sed 's/^/  /' || echo "  (none)"
	@echo ""
	@echo "── IFC declassification anchors (grep -n SQL-DECLASSIFY-) ─────"
	@grep -rn "SQL-DECLASSIFY-001" $(DIR) | sed 's/^/  /' || echo "  (none)"
	@echo ""
	@echo "════════════════════════════════════════════════════════════════"
	@echo "  Three axes required for OWASP A03:2021 verification:"
	@echo "    (1) static proof — universal refinements over user input"
	@echo "    (2) unit tests   — canonical attack payloads exercised"
	@echo "    (3) MC/DC        — compound reject-decision covered"
	@echo "  Runtime obligations (from axis 1) are exercised by axis 2."
	@echo "════════════════════════════════════════════════════════════════"

smoke: guard-mvl ## Smoke-test: build binary (proves compilation without a terminal)
	$(MVL) build $(DIR)main.mvl

run: guard-mvl ## Execute the demo scenarios
	$(MVL) run $(DIR)main.mvl

run-llvm: guard-mvl ## Execute demo scenarios via LLVM backend
	$(MVL) run $(DIR)main.mvl --backend=llvm

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_sql_injection_prevention
