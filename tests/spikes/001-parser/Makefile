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

.PHONY: help build check run tokenize lex compiler check-compiler

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-16s\033[0m %s\n", $$1, $$2}'

build: ## Build the MVL compiler (required before check/run)
	cd "$(ROOT)" && cargo build --quiet

check: ## Type-check experiments 01, 02, and 03
	cd "$(ROOT)" && $(MVL) check tests/spikes/001-parser/01_read_files.mvl
	cd "$(ROOT)" && $(MVL) check tests/spikes/001-parser/02_tokenize_files.mvl
	cd "$(ROOT)" && $(MVL) check tests/spikes/001-parser/03_lex_files.mvl

check-compiler: ## Type-check compiler/ast.mvl + compiler/lexer.mvl + compiler/main.mvl
	cd "$(ROOT)" && $(MVL) check compiler/ast.mvl
	cd "$(ROOT)" && $(MVL) check compiler/lexer.mvl
	cd "$(ROOT)" && $(MVL) check compiler/main.mvl

run: ## Run experiment 01 — reads examples/log_analyzer/main.mvl
	cd "$(ROOT)" && $(MVL) run tests/spikes/001-parser/01_read_files.mvl

tokenize: ## Run experiment 02 — counts tokens in examples/log_analyzer/*.mvl
	cd "$(ROOT)" && $(MVL) run tests/spikes/001-parser/02_tokenize_files.mvl

lex: ## Run experiment 03 — lex files in examples/log_analyzer/*.mvl
	cd "$(ROOT)" && $(MVL) run tests/spikes/001-parser/03_lex_files.mvl

compiler: ## Run compiler/main.mvl — lex the compiler/ source itself
	cd "$(ROOT)" && $(MVL) run compiler/main.mvl

.DEFAULT_GOAL := run
