# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Schuberg Philis
#
# Spike 005 — Span memory and time benchmark (issue #1371)
# Run all targets from the repo root:
#   make -C tests/spikes/005-span-bench
# Or from this directory:
#   make

ROOT := $(shell git -C "$(dir $(abspath $(lastword $(MAKEFILE_LIST))))" rev-parse --show-toplevel)
MVL  := $(ROOT)/target/debug/mvl

.PHONY: help build guard-mvl check bench

.DEFAULT_GOAL := help

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-16s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Validate MVL binary is present (does NOT recompile)
	@$(MVL) --version > /dev/null 2>&1 || { \
		echo ""; \
		echo "  ERROR: MVL compiler not found at: $(MVL)"; \
		echo "  Run 'make build' from the repo root first."; \
		echo ""; \
		exit 1; \
	}

build: ## Build the MVL compiler (required before check/bench)
	cd "$(ROOT)" && cargo build --quiet

check: guard-mvl ## Type-check all spike .mvl files
	cd "$(ROOT)" && $(MVL) check tests/spikes/005-span-bench/01_bench.mvl

bench: guard-mvl ## Run the split vs split_spans benchmark
	cd "$(ROOT)" && $(MVL) run tests/spikes/005-span-bench/01_bench.mvl
