# task_pipeline example — pure MVL CSV data processor (issue #171)
#
# Demonstrates zero-extern pure MVL for a complete data transformation pipeline.
# Effect boundary: only main.mvl carries ! FileRead, ! Console.
# All transform functions in transform.mvl are provably pure.
#
# Run from this directory or from the repo root.
.PHONY: help guard-mvl check lint test-solver test assurance mcdc mutate fuzz run run-threshold all clean

.DEFAULT_GOAL := help

MVL  := ../../target/debug/mvl
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Validate MVL binary is present and working (does NOT recompile)
	@$(MVL) --version > /dev/null 2>&1 || { \
		echo ""; \
		echo "  ERROR: MVL compiler not found at: $(MVL)"; \
		echo "  Run 'make build' from the repo root first."; \
		echo ""; \
		exit 1; \
	}

build: guard-mvl ## Only build the application
	$(MVL) build $(DIR)main.mvl

check: guard-mvl ## Type-check pure source files (main.mvl excluded: uses stdlib I/O with Tainted[String])
	$(MVL) check $(DIR)model.mvl
	$(MVL) check $(DIR)parser.mvl
	$(MVL) check $(DIR)transform.mvl

lint: guard-mvl ## Lint all source files
	$(MVL) lint $(DIR)

test-solver: guard-mvl ## Show per-file solver statistics for all source files
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(DIR)model.mvl $(DIR)parser.mvl $(DIR)transform.mvl; do \
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

test: guard-mvl ## Run all unit tests (parser + transform + main)
	$(MVL) test $(DIR)

assurance: guard-mvl ## Assurance report: spec coverage, MC/DC, mutation score (verbose)
	$(MVL) assurance $(DIR) --verbose

mcdc: guard-mvl ## Run MC/DC coverage analysis (ARGS="--masking" to exempt coupled clauses)
	$(MVL) mcdc $(DIR) --verbose $(ARGS)

mutate: guard-mvl ## Run mutation testing
	$(MVL) mutate $(DIR)

fuzz: guard-mvl ## Type-directed fuzzing of parse_csv (requires nightly + cargo-fuzz; ARGS="--time 60s")
	$(MVL) fuzz $(DIR) --target parse_csv $(ARGS)

run: guard-mvl ## Run the pipeline with the bundled sample CSV
	$(MVL) run $(DIR)main.mvl -- --input $(DIR)sample.csv

run-threshold: guard-mvl ## Run with a 50.0 amount threshold
	$(MVL) run $(DIR)main.mvl -- --input $(DIR)sample.csv --threshold 50.0

all: check test assurance mcdc ## Run full quality pipeline: check + test + assurance + mcdc
	@echo ""
	@echo "Effect boundary check (should produce no output):"
	@grep -n '!' $(DIR)transform.mvl | grep -v '//' || true

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_task_pipeline
