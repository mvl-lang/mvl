# compiler/Makefile — local dev loop for the self-hosted MVL compiler
#
# Run from this directory: `make test`, `make check`, `make build`, etc.
# All targets go through the built MVL binary at ../target/debug/mvl.
#
# The self-hosted compiler modules live here:
#   - lexer.mvl / parser.mvl / tir.mvl        — front end (#187, #1116)
#   - resolver.mvl / mono.mvl / context.mvl   — mid layer
#   - verify_*.mvl                            — checker port (in progress)
#   - backends/llvm/*.mvl                     — LLVM IR emitter (#1118, #1693)
#   - main.mvl                                — driver
#
# See the top-level Makefile for the umbrella `test-mvl` target that runs
# this same discovery from CI.

.PHONY: help guard-mvl check test build lint prove assurance audit all clean \
        test-lexer test-parser test-llvm-backend

.DEFAULT_GOAL := help

MVL  := $(shell git rev-parse --show-toplevel)/target/debug/mvl
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@echo ""
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
	  awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}'
	@echo ""

guard-mvl: ## Validate the MVL binary exists (does NOT recompile)
	@$(MVL) --version > /dev/null 2>&1 || { \
		echo ""; \
		echo "  ERROR: MVL compiler not found at: $(MVL)"; \
		echo "  Run 'make build' from the repo root first."; \
		echo ""; \
		exit 1; \
	}

# ── Fast dev loop ────────────────────────────────────────────────────────────

check: guard-mvl ## Type-check every .mvl file in compiler/ (recursive)
	$(MVL) check $(DIR)

test: guard-mvl ## Run all *_test.mvl files (self-hosted compiler tests)
	$(MVL) test $(DIR)

lint: guard-mvl ## Lint the self-hosted compiler code
	$(MVL) lint $(DIR)

# ── Focused test targets ─────────────────────────────────────────────────────
# Same content as `test`, but scoped so a targeted rerun after touching
# one subsystem stays fast.

test-lexer: guard-mvl ## Just run lexer_test.mvl
	$(MVL) test $(DIR)lexer_test.mvl

test-parser: guard-mvl ## Just run parser_test.mvl
	$(MVL) test $(DIR)parser_test.mvl

test-llvm-backend: guard-mvl ## Just run backends/llvm/*_test.mvl
	$(MVL) test $(DIR)backends/llvm/

# ── Formal analysis ──────────────────────────────────────────────────────────

prove: guard-mvl ## Per-call-site refinement proof breakdown
	$(MVL) prove $(DIR)

assurance: guard-mvl ## Full assurance report (spec coverage, MC/DC, mutation score)
	$(MVL) assurance $(DIR) --verbose

audit: guard-mvl ## Run the standard audit bundle (paradox + supply-chain + license)
	$(MVL) audit

# ── Build ────────────────────────────────────────────────────────────────────

build: guard-mvl ## Build the MVL-hosted LLVM emitter (backends/llvm/emitter.mvl)
	$(MVL) build $(DIR)backends/llvm/emitter.mvl

# ── Aggregate ────────────────────────────────────────────────────────────────

all: check test assurance ## Full quality pipeline: check + test + assurance

# ── Cleanup ──────────────────────────────────────────────────────────────────

clean: ## Remove any transpile/build artifacts left in this tree
	@# Nothing generated in compiler/ today; kept for parity with example Makefiles.
	@echo "compiler/ is clean — no artifacts to remove."
