# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Schuberg Philis
#
# Spike 006 — WASM Backend
#
# Two pipelines:
#
#   Emit (default `test`) — the whole thing:
#       .mvl → mvl build --backend=wasm → .wat → wasm-tools → wasmtime
#     Uses a fresh debug build of mvl as a prerequisite so the emitter under
#     test is always the current source, not a stale binary.
#
#   Reference (opt-in) — hand-written WAT under *_reference.wat, kept as a
#     spec of the target shape the emitter should produce:
#       hand-written .wat → wasm-tools → wasmtime
#
# Run from the repo root:
#   make -C tests/spikes/006-wasm-backend test
# Or from this directory:
#   make test

ROOT       := $(shell git -C "$(dir $(abspath $(lastword $(MAKEFILE_LIST))))" rev-parse --show-toplevel)
MVL        := $(ROOT)/target/debug/mvl
SPIKE      := $(ROOT)/tests/spikes/006-wasm-backend
ADD_SRC    := $(SPIKE)/add.mvl
HELLO_SRC  := $(SPIKE)/hello.mvl

# Hand-written reference WAT (checked in) — target shape the emitter should
# match. Renamed to *_reference.wat to make the role explicit and to avoid
# name collisions with the emitter's output.
ADD_WAT_REF   := $(SPIKE)/add_reference.wat
HELLO_WAT_REF := $(SPIKE)/hello_reference.wat

# Emitted artifacts — written into a scratch dir so `mvl build` (which writes
# `<stem>.wat` in CWD) doesn't clobber the reference files in the repo.
BUILD_DIR  := /tmp/spike006-build
ADD_WAT    := $(BUILD_DIR)/add.wat
ADD_WASM   := $(BUILD_DIR)/add.wasm
HELLO_WAT  := $(BUILD_DIR)/hello.wat
HELLO_WASM := $(BUILD_DIR)/hello.wasm

.PHONY: help mvl guard-wasm check add hello add-reference hello-reference test test-reference all clean

.DEFAULT_GOAL := help

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
	  awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-16s\033[0m %s\n", $$1, $$2}'

# `mvl` is a prerequisite of every target that runs the emitter. It performs
# an incremental debug `cargo build` — a no-op when the tree is already fresh,
# so the price for freshness is negligible and no stale binary can be tested.
mvl: ## Build the mvl debug binary specifically for this spike
	@echo "→ cargo build (debug) for spike 006"
	@cd "$(ROOT)" && cargo build --quiet
	@$(MVL) --version > /dev/null

guard-wasm: ## Check WASM toolchain is installed
	@command -v wasm-tools > /dev/null 2>&1 || { \
	  echo "ERROR: wasm-tools not found — install with 'cargo install wasm-tools'"; exit 1; }
	@command -v wasmtime > /dev/null 2>&1 || { \
	  echo "ERROR: wasmtime not found — see https://wasmtime.dev/"; exit 1; }

$(BUILD_DIR):
	@mkdir -p $(BUILD_DIR)

check: mvl ## Type-check the MVL reference sources
	cd "$(ROOT)" && $(MVL) check $(ADD_SRC)
	cd "$(ROOT)" && $(MVL) check $(HELLO_SRC)

# ── Emit pipeline (the whole thing) ─────────────────────────────────────────

add: mvl guard-wasm $(BUILD_DIR) ## Emit add.wat via mvl, assemble, invoke main + add(7,35)
	@echo "→ mvl build add.mvl --backend=wasm"
	@cd $(BUILD_DIR) && $(MVL) build $(ADD_SRC) --backend=wasm
	@echo "  emitted: $(ADD_WAT)"
	@echo "→ wasm-tools parse"
	@wasm-tools parse $(ADD_WAT) -o $(ADD_WASM)
	@echo "  binary:  $(ADD_WASM)"
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

hello: mvl guard-wasm $(BUILD_DIR) ## Emit hello.wat via mvl, assemble, run via WASI (prints "5")
	@echo "→ mvl build hello.mvl --backend=wasm"
	@cd $(BUILD_DIR) && $(MVL) build $(HELLO_SRC) --backend=wasm
	@echo "  emitted: $(HELLO_WAT)"
	@echo "→ wasm-tools parse"
	@wasm-tools parse $(HELLO_WAT) -o $(HELLO_WASM)
	@echo "  binary:  $(HELLO_WASM)"
	@echo "→ wasmtime run hello.wasm"
	@OUT=$$(wasmtime run $(HELLO_WASM)) && \
	  echo "  stdout: $$OUT" && \
	  if [ "$$OUT" = "5" ]; then echo "  ✓  matches expected: 5"; \
	  else echo "  ✗  expected '5', got '$$OUT'"; exit 1; fi

# ── Reference pipeline (hand-written WAT under *_reference.wat) ─────────────

add-reference: guard-wasm $(BUILD_DIR) ## Assemble hand-written add_reference.wat and invoke main + add(7,35)
	@echo "→ wasm-tools parse (reference) $(ADD_WAT_REF)"
	@wasm-tools parse $(ADD_WAT_REF) -o $(BUILD_DIR)/add_reference.wasm
	@wasmtime run --invoke main $(BUILD_DIR)/add_reference.wasm 2>/dev/null | \
	  awk '{ if ($$1=="5") print "  ✓  main() = 5"; else { print "  ✗  main() = " $$1; exit 1 } }'
	@wasmtime run --invoke add $(BUILD_DIR)/add_reference.wasm 7 35 2>/dev/null | \
	  awk '{ if ($$1=="42") print "  ✓  add(7,35) = 42"; else { print "  ✗  add(7,35) = " $$1; exit 1 } }'

hello-reference: guard-wasm $(BUILD_DIR) ## Assemble hand-written hello_reference.wat and run via WASI
	@echo "→ wasm-tools parse (reference) $(HELLO_WAT_REF)"
	@wasm-tools parse $(HELLO_WAT_REF) -o $(BUILD_DIR)/hello_reference.wasm
	@wasmtime run $(BUILD_DIR)/hello_reference.wasm | \
	  awk '{ if ($$1=="5") print "  ✓  stdout = 5"; else { print "  ✗  stdout = " $$1; exit 1 } }'

# ── Aggregate targets ───────────────────────────────────────────────────────

test: check add hello ## Full emit pipeline: check + emit + assemble + run for both programs
	@echo ""
	@echo "✓  spike 006 emit pipeline passes (add + hello)"

test-reference: check add-reference hello-reference ## Reference pipeline against hand-written WAT
	@echo ""
	@echo "✓  spike 006 hand-written reference pipeline passes"

all: test test-reference ## Run both pipelines

clean: ## Remove generated build artifacts
	rm -rf $(BUILD_DIR)
