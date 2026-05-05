# medical_triage example — MC/DC coverage demonstration (IEC 62304)
# Run from this directory or from the repo root.
.PHONY: help guard-mvl check test test-llvm mcdc mutate run run-llvm all clean

MVL  := $(shell test -x ../../target/debug/mvl && echo ../../target/debug/mvl || echo mvl)
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Validate MVL binary is present and working (does NOT recompile)
	@$(MVL) --version > /dev/null 2>&1 || { \
		echo ""; \
		echo "  ERROR: MVL compiler not found at: $(MVL)"; \
		echo "  Run 'make build' from the repo root first."; \
		echo ""; \
		exit 1; \
	}

check: guard-mvl ## Type-check all source files
	$(MVL) check $(DIR)model.mvl
	$(MVL) check $(DIR)triage.mvl
	$(MVL) check $(DIR)main.mvl

test: guard-mvl ## Run unit tests
	$(MVL) test $(DIR)

test-llvm: guard-mvl ## Run unit tests via LLVM backend
	$(MVL) test $(DIR) --backend=llvm

mcdc: guard-mvl ## Run MC/DC coverage analysis (use ARGS="--masking" to exempt coupled)
	$(MVL) mcdc $(DIR) $(ARGS)

mutate: guard-mvl ## Run mutation testing
	$(MVL) mutate $(DIR)

all: check test mcdc ## Run full quality pipeline: check + test + mcdc

run: guard-mvl ## Execute the demo scenarios
	$(MVL) run $(DIR)main.mvl

run-llvm: guard-mvl ## Execute demo scenarios via LLVM backend
	$(MVL) run $(DIR)main.mvl --backend=llvm

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_medical_triage
