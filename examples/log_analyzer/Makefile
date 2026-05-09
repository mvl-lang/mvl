# log_analyzer example — MVL Phase 4 demonstration
# Run from this directory: make build, make test, etc.
.PHONY: help guard-mvl build check test test-llvm generate run run-llvm clean

MVL  := ../../target/debug/mvl
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

build: guard-mvl ## Compile the example to a native binary
	$(MVL) build $(DIR)main.mvl

check: guard-mvl ## Type-check all three source files
	$(MVL) check $(DIR)main.mvl
	$(MVL) check $(DIR)parser.mvl
	$(MVL) check $(DIR)utils.mvl

test: guard-mvl ## Run all standalone _test.mvl files (18 tests)
	$(MVL) test $(DIR)

test-llvm: guard-mvl ## Run tests via LLVM backend
	$(MVL) test $(DIR) --backend=llvm

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
