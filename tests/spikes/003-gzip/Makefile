# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Schuberg Philis
#
# Spike 003 — gzip compress/decompress in pure MVL  (issue #1256)
# Run all targets from the repo root:
#   make -C tests/spikes/003-gzip
# Or from this directory:
#   make

ROOT := $(shell git -C "$(dir $(abspath $(lastword $(MAKEFILE_LIST))))" rev-parse --show-toplevel)
MVL  := $(ROOT)/target/debug/mvl
DIR  := tests/spikes/003-gzip

.PHONY: help build guard-mvl check test test-lz77 test-huffman test-deflate test-gzip test-perf

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

build: ## Build the MVL compiler (required before check/test)
	cd "$(ROOT)" && cargo build --quiet

check: guard-mvl ## Type-check all .mvl files in this spike
	cd "$(ROOT)" && for f in $(DIR)/*.mvl; do echo "  check $$f"; $(MVL) check "$$f"; done

test: guard-mvl ## Run all unit tests in this spike
	cd "$(ROOT)" && $(MVL) test $(DIR)/

test-lz77: guard-mvl ## Run LZ77 unit tests
	cd "$(ROOT)" && $(MVL) test $(DIR)/lz77.mvl

test-huffman: guard-mvl ## Run fixed-Huffman unit tests
	cd "$(ROOT)" && $(MVL) test $(DIR)/huffman_fixed.mvl

test-deflate: guard-mvl ## Run DEFLATE roundtrip tests
	cd "$(ROOT)" && $(MVL) test $(DIR)/deflate.mvl

test-gzip: guard-mvl ## Run full gzip functional tests
	cd "$(ROOT)" && $(MVL) test $(DIR)/gzip_test.mvl

test-perf: guard-mvl ## Run performance / ratio tests
	cd "$(ROOT)" && $(MVL) test $(DIR)/gzip_perf.mvl
