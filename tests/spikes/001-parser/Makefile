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

.PHONY: help build check run

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-8s\033[0m %s\n", $$1, $$2}'

build: ## Build the MVL compiler (required before check/run)
	cd "$(ROOT)" && cargo build --quiet

check: ## Type-check experiment 01
	cd "$(ROOT)" && $(MVL) check tests/spikes/001-parser/01_read_files.mvl

run: ## Run experiment 01 — reads examples/log_analyzer/main.mvl
	cd "$(ROOT)" && $(MVL) run tests/spikes/001-parser/01_read_files.mvl

.DEFAULT_GOAL := run
