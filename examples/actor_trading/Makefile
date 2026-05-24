# actor_trading — verified order-matching engine (issue #582)
#
# Combines Phase 8 actors with Req 10 refinement types.
# Scalar predicates (price > 0, quantity > 0) are proved statically by the
# MVL layered solver.  Cross-field invariants (no-worse-than-limit, quantity
# conservation) are proved for ALL inputs by the Z3 harness in verify.py.
.PHONY: help guard-mvl guard-z3 lint check test test-solver test-llvm assurance verify smoke run clean

.DEFAULT_GOAL := help

MVL  := ../../target/debug/mvl
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Ensure MVL binary is built and up-to-date
	@cargo build --manifest-path $(DIR)../../Cargo.toml --quiet

guard-z3: ## Validate z3 Python package is available
	@python3 -c "import z3" 2>/dev/null || { \
		echo ""; \
		echo "  ERROR: z3 Python package not found."; \
		echo "  Install with: pip install z3-solver"; \
		echo ""; \
		exit 1; \
	}

lint: guard-mvl ## Lint all source files
	$(MVL) lint $(DIR)types.mvl
	$(MVL) lint $(DIR)main.mvl

check: guard-mvl ## Type-check types.mvl and main.mvl
	$(MVL) check $(DIR)types.mvl
	$(MVL) check $(DIR)main.mvl

test: guard-mvl ## Run unit tests
	$(MVL) test $(DIR)

test-solver: check ## Show per-file solver statistics (alias for check)

test-llvm: guard-mvl ## Run tests via LLVM backend
	$(MVL) test $(DIR) --backend=llvm

assurance: guard-mvl ## Assurance report (verbose)
	$(MVL) assurance $(DIR) --verbose

verify: guard-z3 ## Prove invariants for ALL inputs using Z3
	python3 $(DIR)verify.py

smoke: guard-mvl ## Smoke-test: build binary (proves compilation without a terminal)
	$(MVL) build $(DIR)main.mvl

run: guard-mvl ## Run the three trading scenarios
	$(MVL) run $(DIR)main.mvl

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_actor_trading
