# flight_clearance example — MC/DC coverage demonstration
# Run from this directory or from the repo root.
.PHONY: help guard-mvl check lint test-solver test test-llvm assurance mcdc mutate smoke run run-llvm all clean

.DEFAULT_GOAL := help

MVL  := ../../target/debug/mvl
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Ensure MVL binary is built and up-to-date
	@cargo build --manifest-path $(DIR)../../Cargo.toml --quiet

check: guard-mvl ## Type-check all source files
	$(MVL) check $(DIR)model.mvl
	$(MVL) check $(DIR)clearance.mvl
	$(MVL) check $(DIR)main.mvl

lint: guard-mvl ## Lint all source files
	$(MVL) lint $(DIR)

test-solver: guard-mvl ## Show per-file solver statistics for all source files
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(DIR)model.mvl $(DIR)clearance.mvl $(DIR)main.mvl; do \
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

mcdc: guard-mvl ## Run MC/DC coverage analysis (56/57 unique-cause; coupled clause exempt)
	$(MVL) mcdc $(DIR) --verbose --masking $(ARGS)

mutate: guard-mvl ## Run mutation testing
	$(MVL) mutate $(DIR)

assurance: guard-mvl ## Assurance report: spec coverage, MC/DC, mutation score (verbose)
	$(MVL) assurance $(DIR) --verbose

all: check test assurance mcdc ## Run full quality pipeline: check + test + assurance + mcdc

smoke: guard-mvl ## Smoke-test: build binary (proves compilation without a terminal)
	$(MVL) build $(DIR)main.mvl

run: guard-mvl ## Execute the demo scenarios
	$(MVL) run $(DIR)main.mvl

run-llvm: guard-mvl ## Execute demo scenarios via LLVM backend
	$(MVL) run $(DIR)main.mvl --backend=llvm

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_flight_clearance
