# log_analyzer example — MVL Phase 4 demonstration
# Run from this directory: make build, make test, etc.
.PHONY: help guard-mvl build check prove lint test-solver test test-llvm test-wasm assurance mcdc mutate fuzz generate smoke run run-llvm run-wasm all clean

.DEFAULT_GOAL := help

MVL  := ../../target/debug/mvl
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
WASM_RUNTIME := ../../target/wasm32-wasip1/debug/mvl_runtime_wasm.wasm

# $(MVL) always points at the freshly-built local binary above, never an
# installed toolchain — so the re-exec `mvl` does by default (jumping to a
# project-pinned installed version per `requires-mvl`) is never what we
# want here. Without this, every target silently tests whatever version
# happens to be installed under XDG_DATA_HOME instead of this checkout.
export MVL_NO_REEXEC := 1

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

check: guard-mvl ## Type-check all source files
	$(MVL) check $(DIR)

prove: guard-mvl ## Prove per-call-site refinement proof layer breakdown
	$(MVL) prove $(DIR)

lint: guard-mvl ## Lint all source files
	$(MVL) lint $(DIR)

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

test-llvm: guard-mvl ## Run tests via --backend=llvm
	$(MVL) test $(DIR) --backend=llvm

test-wasm: guard-mvl ## Run tests via --backend=wasm
	MVL_RUNTIME_WASM=$(WASM_RUNTIME) $(MVL) test $(DIR) --backend=wasm


assurance: guard-mvl ## Assurance report: spec coverage, MC/DC, mutation score (verbose)
	$(MVL) assurance $(DIR) --verbose

mcdc: guard-mvl ## Run MC/DC coverage analysis (ARGS="--masking" to exempt coupled clauses)
	$(MVL) mcdc $(DIR) --verbose $(ARGS)

mutate: guard-mvl ## Run mutation testing
	$(MVL) mutate $(DIR)

fuzz: guard-mvl ## Type-directed fuzzing of parse_level / parse_line (requires nightly + cargo-fuzz; ARGS="--time 60s")
	$(MVL) fuzz $(DIR) $(ARGS)

all: check test assurance mcdc ## Run full quality pipeline: check + test + assurance + mcdc

generate: ## Generate sample log data (logs.jsonl, 200 entries)
	python3 $(DIR)log_generator.py -o logs.jsonl

smoke: guard-mvl ## Smoke-test: build binary (proves compilation without a terminal)
	$(MVL) build $(DIR)main.mvl

run: guard-mvl logs.jsonl ## Analyze logs.jsonl (requires: make generate)
	$(MVL) run $(DIR)main.mvl -- --file logs.jsonl

run-llvm: guard-mvl logs.jsonl ## Analyze logs.jsonl via LLVM backend (requires: make generate)
	$(MVL) run $(DIR)main.mvl --backend=llvm -- --file logs.jsonl

# `mvl run --backend=wasm` is rejected outright by the CLI ("use `mvl build
# --backend=wasm` (via wasmtime) or `mvl test --backend=wasm`") — this
# mirrors tools/mvlr's own build -> wasm-tools parse -> wasmtime run chain
# for a single fn-main file, since there is no built-in `mvl run-wasm`.
# `--dir` grants the module access to the cwd so `read_file` can see
# logs.jsonl.
run-wasm: guard-mvl logs.jsonl ## Analyze logs.jsonl via WASM backend (requires: make generate; wasmtime + wasm-tools)
	@tmp=$$(mktemp -d) || exit 1; \
	trap 'rm -rf "$$tmp"' EXIT; \
	(cd "$$tmp" && $(abspath $(MVL)) build $(DIR)main.mvl --backend=wasm) || exit 1; \
	wasm-tools parse "$$tmp/main.wat" -o "$$tmp/main.wasm" || exit 1; \
	wasmtime run --dir=$(DIR)::. --preload runtime=$(abspath $(WASM_RUNTIME)) "$$tmp/main.wasm" -- --file logs.jsonl

logs.jsonl:
	$(MAKE) generate

clean: ## Remove generated log data and build artifacts
	rm -f logs.jsonl
	rm -rf $(TMPDIR)mvl_build_log_analyzer
