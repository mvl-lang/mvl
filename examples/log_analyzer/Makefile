# log_analyzer example — MVL Phase 4 demonstration
# Run from this directory: make build, make test, etc.
.PHONY: help build check test generate run clean

MVL  := $(shell test -x ../../target/debug/mvl && echo ../../target/debug/mvl || echo mvl)
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

build: ## Compile the example to a native binary
	$(MVL) build $(DIR)main.mvl

check: ## Type-check all three source files
	$(MVL) check $(DIR)main.mvl
	$(MVL) check $(DIR)parser.mvl
	$(MVL) check $(DIR)utils.mvl

test: ## Run all standalone _test.mvl files (18 tests)
	$(MVL) test $(DIR)

generate: ## Generate sample log data (logs.jsonl, 200 entries)
	python3 $(DIR)log_generator.py -o logs.jsonl

run: logs.jsonl ## Analyze logs.jsonl (requires: make generate)
	$(MVL) run $(DIR)main.mvl -- --file logs.jsonl

logs.jsonl:
	$(MAKE) generate

clean: ## Remove generated log data and build artifacts
	rm -f logs.jsonl
	rm -rf $(TMPDIR)mvl_build_log_analyzer
