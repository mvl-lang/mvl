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

.PHONY: help build check check-compiler run-01 run-02 run-03 run-main

.DEFAULT_GOAL := help

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-16s\033[0m %s\n", $$1, $$2}'

build: ## Build the MVL compiler (required before check/run-*)
	cd "$(ROOT)" && cargo build --quiet

check: ## Type-check all experiment .mvl files (0N_*.mvl)
	cd "$(ROOT)" && for f in tests/spikes/001-parser/[0-9][0-9]_*.mvl; do $(MVL) check "$$f"; done

check-compiler: ## Type-check all compiler/ .mvl files
	cd "$(ROOT)" && for f in compiler/*.mvl; do $(MVL) check "$$f"; done

run-01: ## Run experiment 01 — reads examples/log_analyzer/main.mvl
	cd "$(ROOT)" && $(MVL) run tests/spikes/001-parser/01_read_files.mvl

run-02: ## Run experiment 02 — counts tokens in examples/log_analyzer/*.mvl
	cd "$(ROOT)" && $(MVL) run tests/spikes/001-parser/02_tokenize_files.mvl

run-03: ## Run experiment 03 — lex files in examples/log_analyzer/*.mvl
	cd "$(ROOT)" && $(MVL) run tests/spikes/001-parser/03_lex_files.mvl

run-main: ## Run $(ARGS) through the MVL lexer (override: make run-main ARGS=path/to/file.mvl)
	cd "$(ROOT)" && $(MVL) run $(ARGS)
