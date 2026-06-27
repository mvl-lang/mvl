# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Schuberg Philis
#
# Spike 006 — WASM Backend
# Pipeline: MVL source (reference) → hand-written WAT → wasm-tools → wasmtime
#
# Two variants:
#   add.{mvl,wat}    — pure compute, exported functions, no host imports
#   hello.{mvl,wat}  — WASI preview 1 fd_write (host-import story)
#
# Run all targets from the repo root:
#   make -C tests/spikes/006-wasm-backend
# Or from this directory:
#   make

ROOT     := $(shell git -C "$(dir $(abspath $(lastword $(MAKEFILE_LIST))))" rev-parse --show-toplevel)
MVL      := $(ROOT)/target/debug/mvl
SPIKE    := $(ROOT)/tests/spikes/006-wasm-backend
ADD_SRC    := $(SPIKE)/add.mvl
ADD_WAT    := $(SPIKE)/add.wat
ADD_WASM   := /tmp/spike006_add.wasm
HELLO_SRC  := $(SPIKE)/hello.mvl
HELLO_WAT  := $(SPIKE)/hello.wat
HELLO_WASM := /tmp/spike006_hello.wasm

.PHONY: help build guard-mvl guard-wasm check add hello test all clean

.DEFAULT_GOAL := help

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
	  awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Check MVL binary exists
	@$(MVL) --version > /dev/null 2>&1 || { \
	  echo "ERROR: $(MVL) not found — run 'make build' first"; exit 1; }

guard-wasm: ## Check WASM toolchain is installed
	@command -v wasm-tools > /dev/null 2>&1 || { \
	  echo "ERROR: wasm-tools not found — install with 'cargo install wasm-tools'"; exit 1; }
	@command -v wasmtime > /dev/null 2>&1 || { \
	  echo "ERROR: wasmtime not found — see https://wasmtime.dev/"; exit 1; }

build: ## Build the MVL compiler
	cd "$(ROOT)" && cargo build --quiet

check: guard-mvl ## Type-check the MVL reference sources
	cd "$(ROOT)" && $(MVL) check $(ADD_SRC)
	cd "$(ROOT)" && $(MVL) check $(HELLO_SRC)

add: guard-wasm ## Assemble add.wat and invoke main + add(7, 35)
	@echo "→ wasm-tools parse add.wat"
	@wasm-tools parse $(ADD_WAT) -o $(ADD_WASM)
	@echo "  binary: $(ADD_WASM)"
	@echo "→ wasmtime --invoke main"
	@OUT=$$(wasmtime run --invoke main $(ADD_WASM) 2>/dev/null) && \
	  echo "  main() = $$OUT" && \
	  if [ "$$OUT" = "5" ]; then echo "  ✓  matches expected: 5"; \
	  else echo "  ✗  expected '5', got '$$OUT'"; exit 1; fi
	@echo "→ wasmtime --invoke add 7 35"
	@OUT=$$(wasmtime run --invoke add $(ADD_WASM) 7 35 2>/dev/null) && \
	  echo "  add(7, 35) = $$OUT" && \
	  if [ "$$OUT" = "42" ]; then echo "  ✓  matches expected: 42"; \
	  else echo "  ✗  expected '42', got '$$OUT'"; exit 1; fi

hello: guard-wasm ## Assemble hello.wat and run via WASI (prints "5")
	@echo "→ wasm-tools parse hello.wat"
	@wasm-tools parse $(HELLO_WAT) -o $(HELLO_WASM)
	@echo "  binary: $(HELLO_WASM)"
	@echo "→ wasmtime run hello.wasm"
	@OUT=$$(wasmtime run $(HELLO_WASM)) && \
	  echo "  stdout: $$OUT" && \
	  if [ "$$OUT" = "5" ]; then echo "  ✓  matches expected: 5"; \
	  else echo "  ✗  expected '5', got '$$OUT'"; exit 1; fi

test: check add hello ## Full spike — check, assemble, run, verify outputs

all: test ## Alias for test

clean: ## Remove generated wasm artifacts
	rm -f $(ADD_WASM) $(HELLO_WASM)
