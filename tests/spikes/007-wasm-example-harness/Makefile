SHELL := /bin/bash
FILE ?= test-cases/option_probe/main.mvl

.PHONY: help build run test clean

.DEFAULT_GOAL := help

help: ## Show this help
	@echo ""
	@awk 'BEGIN {FS = ":.*?## "} \
	  /^# === .* ===$$/  { sub(/^# === /, ""); sub(/ ===$$/, ""); printf "\n\033[33m%s\033[0m\n", $$0 } \
	  /^[a-zA-Z0-9_-]+:.*?## / { printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2 }' \
	  $(MAKEFILE_LIST)
	@echo ""

# === Build ===

build: ## Install harness/ dependencies (@bjorn3/browser_wasi_shim)
	cd harness && npm install --no-audit --no-fund

# === Run ===

run: build ## Compile + execute one .mvl file through the WASM harness (vars: FILE)
	node harness/run-example.mjs $(FILE)

# === Test ===

test: build ## Run every test-cases/<name>/ through the harness, diff against expected-stdout.txt
	./run-tests.sh

# === Clean ===

clean: ## Remove harness/ dependencies
	rm -rf harness/node_modules
