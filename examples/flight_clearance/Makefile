# flight_clearance example — MC/DC coverage demonstration
# Run from this directory or from the repo root.
.PHONY: help check test test-llvm mcdc mutate run run-llvm all clean

MVL  := $(shell test -x ../../target/debug/mvl && echo ../../target/debug/mvl || echo mvl)
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

check: ## Type-check all source files
	$(MVL) check $(DIR)model.mvl
	$(MVL) check $(DIR)clearance.mvl
	$(MVL) check $(DIR)main.mvl

test: ## Run unit tests
	$(MVL) test $(DIR)

test-llvm: ## Run unit tests via LLVM backend
	$(MVL) test $(DIR) --backend=llvm

mcdc: ## Run MC/DC coverage analysis (use ARGS="--masking" to exempt coupled)
	$(MVL) mcdc $(DIR) $(ARGS)

mutate: ## Run mutation testing
	$(MVL) mutate $(DIR)

all: check test mcdc ## Run full quality pipeline: check + test + mcdc

run: ## Execute the demo scenarios
	$(MVL) run $(DIR)main.mvl

run-llvm: ## Execute demo scenarios via LLVM backend
	$(MVL) run $(DIR)main.mvl --backend=llvm

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_flight_clearance
