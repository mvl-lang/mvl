# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Schuberg Philis
#
# Spike 001 — Parser in MVL  (issue #187)
# Run all targets from the repo root:
#   make -C tests/spikes/001-parser
# Or from this directory:
#   make

ROOT := $(shell git -C "$(dir $(abspath $(lastword $(MAKEFILE_LIST))))" rev-parse --show-toplevel)
MVL  := $(ROOT)/target/debug/mvl
ARGS ?= compiler/main.mvl

.PHONY: help build guard-mvl check check-compiler test \
        test-01 test-02 test-03 test-04 test-05 test-06 test-07 test-08 test-09 \
        run-01 run-02 run-03 run-04 run-05 run-06 run-07 run-08 run-09 run-main

.DEFAULT_GOAL := help

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-16s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Validate MVL binary is present and working (does NOT recompile)
	@$(MVL) --version > /dev/null 2>&1 || { \
		echo ""; \
		echo "  ERROR: MVL compiler not found at: $(MVL)"; \
		echo "  Run 'make build' from the repo root first."; \
		echo ""; \
		exit 1; \
	}

build: ## Build the MVL compiler (required before check/run-*/test-*)
	cd "$(ROOT)" && cargo build --quiet

check: guard-mvl ## Type-check all experiment .mvl files (0N_*.mvl)
	cd "$(ROOT)" && for f in tests/spikes/001-parser/[0-9][0-9]_*.mvl; do $(MVL) check "$$f"; done

check-compiler: guard-mvl ## Type-check all compiler/ .mvl files
	cd "$(ROOT)" && for f in compiler/*.mvl; do $(MVL) check "$$f"; done

test: guard-mvl ## Run all unit tests (*_test.mvl) in this spike
	cd "$(ROOT)" && $(MVL) test tests/spikes/001-parser/

test-01: guard-mvl ## Run unit tests for experiment 01 (file/path helpers)
	cd "$(ROOT)" && $(MVL) test tests/spikes/001-parser/01_read_test.mvl

test-02: guard-mvl ## Run unit tests for experiment 02 (token counting)
	cd "$(ROOT)" && $(MVL) test tests/spikes/001-parser/02_tokenize_test.mvl

test-03: guard-mvl ## Run unit tests for experiment 03 (lexer classifiers)
	cd "$(ROOT)" && $(MVL) test tests/spikes/001-parser/03_lex_test.mvl

test-04: guard-mvl ## Run unit tests for experiment 04 (parser pure functions)
	cd "$(ROOT)" && $(MVL) test tests/spikes/001-parser/04_parser_test.mvl

test-05: guard-mvl ## Run unit tests for experiment 05 (depth-aware parser)
	cd "$(ROOT)" && $(MVL) test tests/spikes/001-parser/05_depth_test.mvl

test-06: guard-mvl ## Run unit tests for experiment 06 (modifier-aware parser)
	cd "$(ROOT)" && $(MVL) test tests/spikes/001-parser/06_mods_test.mvl

test-07: guard-mvl ## Run unit tests for experiment 07 (use-path parser)
	cd "$(ROOT)" && $(MVL) test tests/spikes/001-parser/07_use_test.mvl

test-08: guard-mvl ## Run unit tests for experiment 08 (signature parser)
	cd "$(ROOT)" && $(MVL) test tests/spikes/001-parser/08_sig_test.mvl

test-09: guard-mvl ## Run unit tests for experiment 09 (complete recursive-descent parser)
	cd "$(ROOT)" && $(MVL) test tests/spikes/001-parser/09_parser_test.mvl

run-01: guard-mvl ## Run experiment 01 — reads examples/log_analyzer/main.mvl
	cd "$(ROOT)" && $(MVL) run tests/spikes/001-parser/01_read_files.mvl

run-02: guard-mvl ## Run experiment 02 — counts tokens in examples/log_analyzer/*.mvl
	cd "$(ROOT)" && $(MVL) run tests/spikes/001-parser/02_tokenize_files.mvl

run-03: guard-mvl ## Run experiment 03 — lex files in examples/log_analyzer/*.mvl
	cd "$(ROOT)" && $(MVL) run tests/spikes/001-parser/03_lex_files.mvl

run-04: guard-mvl ## Run experiment 04 — parse top-level decls in examples/log_analyzer/*.mvl
	cd "$(ROOT)" && $(MVL) run tests/spikes/001-parser/04_parser.mvl

run-05: guard-mvl ## Run experiment 05 — depth-aware parser on examples/log_analyzer/*.mvl
	cd "$(ROOT)" && $(MVL) run tests/spikes/001-parser/05_depth_parser.mvl

run-06: guard-mvl ## Run experiment 06 — modifier-aware parser on examples/log_analyzer/*.mvl
	cd "$(ROOT)" && $(MVL) run tests/spikes/001-parser/06_mods_parser.mvl

run-07: guard-mvl ## Run experiment 07 — use-path parser on examples/log_analyzer/*.mvl
	cd "$(ROOT)" && $(MVL) run tests/spikes/001-parser/07_use_parser.mvl

run-08: guard-mvl ## Run experiment 08 — signature parser on examples/log_analyzer/*.mvl
	cd "$(ROOT)" && $(MVL) run tests/spikes/001-parser/08_sig_parser.mvl

run-09: guard-mvl ## Run experiment 09 — self-hosted parser on compiler/ (dog-food)
	cd "$(ROOT)" && $(MVL) run compiler/main.mvl

run-main: guard-mvl ## Run $(ARGS) through the MVL lexer (override: make run-main ARGS=path/to/file.mvl)
	cd "$(ROOT)" && $(MVL) run $(ARGS)
