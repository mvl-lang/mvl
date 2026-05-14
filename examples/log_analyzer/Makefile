# log_analyzer example — MVL Phase 4 demonstration
# Run from this directory: make build, make test, etc.
.PHONY: help guard-mvl build check test-solver test test-llvm assurance mcdc mutate generate run run-llvm all clean

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

build: guard-mvl ## Compile the example to a native binary
	$(MVL) build $(DIR)main.mvl

check: guard-mvl ## Type-check all three source files
	$(MVL) check $(DIR)main.mvl
	$(MVL) check $(DIR)parser.mvl
	$(MVL) check $(DIR)utils.mvl

test-solver: guard-mvl ## Show per-file solver statistics for all source files
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(DIR)main.mvl $(DIR)parser.mvl $(DIR)utils.mvl; do \
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

test: guard-mvl ## Run all standalone _test.mvl files (18 tests)
	$(MVL) test $(DIR)

test-llvm: guard-mvl ## Run tests via LLVM backend
	$(MVL) test $(DIR) --backend=llvm

assurance: guard-mvl ## Assurance report: spec coverage, MC/DC, mutation score (verbose)
	$(MVL) assurance $(DIR) --verbose

mcdc: guard-mvl ## Run MC/DC coverage analysis (ARGS="--masking" to exempt coupled clauses)
	$(MVL) mcdc $(DIR) --verbose $(ARGS)

mutate: guard-mvl ## Run mutation testing
	$(MVL) mutate $(DIR)

all: check test assurance mcdc ## Run full quality pipeline: check + test + assurance + mcdc

generate: ## Generate sample log data (logs.jsonl, 200 entries)
	python3 $(DIR)log_generator.py -o logs.jsonl

run: guard-mvl logs.jsonl ## Analyze logs.jsonl (requires: make generate)
	$(MVL) run $(DIR)main.mvl -- --file logs.jsonl

run-llvm: guard-mvl logs.jsonl ## Analyze logs.jsonl via LLVM backend (requires: make generate)
	$(MVL) run $(DIR)main.mvl --backend=llvm -- --file logs.jsonl

logs.jsonl:
	$(MAKE) generate

clean: ## Remove generated log data and build artifacts
	rm -f logs.jsonl
	rm -rf $(TMPDIR)mvl_build_log_analyzer
