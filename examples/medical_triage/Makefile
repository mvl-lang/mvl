# medical_triage example — MC/DC coverage demonstration (IEC 62304)
# Run from this directory or from the repo root.
.PHONY: help check test mcdc mutate run all clean

MVL  := $(shell test -x ../../target/debug/mvl && echo ../../target/debug/mvl || echo mvl)
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

check: ## Type-check all source files
	$(MVL) check $(DIR)model.mvl
	$(MVL) check $(DIR)triage.mvl
	$(MVL) check $(DIR)main.mvl

test: ## Run unit tests
	$(MVL) test $(DIR)

mcdc: ## Run MC/DC coverage analysis (use ARGS="--masking" to exempt coupled)
	$(MVL) mcdc $(DIR) $(ARGS)

mutate: ## Run mutation testing
	$(MVL) mutate $(DIR)

all: check test mcdc ## Run full quality pipeline: check + test + mcdc

run: ## Execute the demo scenarios
	$(MVL) run $(DIR)main.mvl

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_medical_triage
