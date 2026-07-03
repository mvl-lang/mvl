# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Schuberg Philis
#
# Spike 004 — TIR Backend Stage A
# Pipeline: mvl tir → JSON → MVL Rust emitter → rustc → binary
#
# Run all targets from the repo root:
#   make -C tests/spikes/004-tir-backend
# Or from this directory:
#   make

ROOT  := $(shell git -C "$(dir $(abspath $(lastword $(MAKEFILE_LIST))))" rev-parse --show-toplevel)
MVL   := $(ROOT)/target/debug/mvl
SPIKE := $(ROOT)/tests/spikes/004-tir-backend
SRC   := $(SPIKE)/hello.mvl
EMIT  := $(SPIKE)/backend_rust.mvl
TIR   := /tmp/spike004_tir.json
RS    := /tmp/spike004_out.rs
BIN   := /tmp/spike004_bin

# LLVM path (A0 — issue #1118).  Uses empty.mvl (fn main -> Int { 0 }) with
# a canned exit-0 module emitted by the MVL-hosted backend under
# compiler/backends/llvm/.
SRC_LL  := $(SPIKE)/empty.mvl
EMIT_LL := $(ROOT)/compiler/backends/llvm/backend_llvm.mvl
TIR_LL  := /tmp/spike004_tir_llvm.json
LL      := /tmp/spike004_out.ll
ASM_LL  := /tmp/spike004_out.s
BIN_LL  := /tmp/spike004_bin_llvm

.PHONY: help build guard-mvl tir emit compile run test check all clean \
        tir-llvm emit-llvm compile-llvm run-llvm test-llvm llvm

.DEFAULT_GOAL := help

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
	  awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Check MVL binary exists
	@$(MVL) --version > /dev/null 2>&1 || { \
	  echo "ERROR: $(MVL) not found — run 'make build' first"; exit 1; }

build: ## Build the MVL compiler
	cd "$(ROOT)" && cargo build --quiet

check: guard-mvl ## Type-check hello.mvl and backend_rust.mvl
	cd "$(ROOT)" && $(MVL) check $(SRC)
	cd "$(ROOT)" && $(MVL) check $(EMIT)

tir: guard-mvl ## Step 1 — Emit TirProgram JSON from hello.mvl
	@echo "→ mvl tir hello.mvl"
	cd "$(ROOT)" && $(MVL) tir $(SRC) > $(TIR)
	@echo "  written: $(TIR)"
	@echo "  fns: $$(python3 -c 'import json; d=json.load(open("$(TIR)")); print(len(d["fns"]))')"

emit: tir ## Step 2 — Run MVL Rust emitter on TIR JSON
	@echo "→ mvl run backend_rust.mvl < tir.json"
	cd "$(ROOT)" && $(MVL) run $(EMIT) < $(TIR) 2>/dev/null \
	  | grep -v "^Transpiled to:\|^Running: cargo" > $(RS)
	@echo "  written: $(RS)"
	@cat $(RS)

compile: emit ## Step 3 — Compile emitted Rust with rustc
	@echo "→ rustc $(RS)"
	rustc $(RS) -o $(BIN)
	@echo "  binary: $(BIN)"

run: compile ## Step 4 — Run compiled binary
	@echo "→ $(BIN)"
	@echo "  output:"
	@$(BIN)

test: run ## Full pipeline test — check output == "5"
	@ACTUAL=$$($(BIN)) && \
	  if [ "$$ACTUAL" = "5" ]; then \
	    echo "  ✓  output matches expected: '5'"; \
	  else \
	    echo "  ✗  expected '5', got '$$ACTUAL'"; exit 1; \
	  fi

all: test ## Run the complete pipeline and verify output

clean: ## Remove generated files
	rm -f $(TIR) $(RS) $(BIN) $(TIR_LL) $(LL) $(ASM_LL) $(BIN_LL)

# ── LLVM pipeline (A0 scaffold — issue #1118) ────────────────────────────────

tir-llvm: guard-mvl ## LLVM step 1 — emit TirProgram JSON from empty.mvl
	@echo "→ mvl tir empty.mvl"
	cd "$(ROOT)" && $(MVL) tir $(SRC_LL) > $(TIR_LL)
	@echo "  written: $(TIR_LL)"

emit-llvm: tir-llvm ## LLVM step 2 — run MVL LLVM emitter on TIR JSON
	@echo "→ mvl run backend_llvm.mvl < tir.json"
	cd "$(ROOT)" && $(MVL) run $(EMIT_LL) < $(TIR_LL) 2>/dev/null \
	  | grep -v "^Transpiled to:\|^Running: cargo" > $(LL)
	@echo "  written: $(LL)"
	@cat $(LL)

compile-llvm: emit-llvm ## LLVM step 3 — llc + cc → binary
	@echo "→ llc $(LL)"
	llc $(LL) -o $(ASM_LL)
	@echo "→ cc $(ASM_LL)"
	cc $(ASM_LL) -o $(BIN_LL)
	@echo "  binary: $(BIN_LL)"

run-llvm: compile-llvm ## LLVM step 4 — run compiled binary
	@echo "→ $(BIN_LL)"
	@$(BIN_LL); echo "  exit=$$?"

test-llvm: compile-llvm ## Full LLVM pipeline test — check exit code == 0
	@$(BIN_LL); RC=$$?; \
	  if [ "$$RC" = "0" ]; then \
	    echo "  ✓  binary exited with code 0 as expected"; \
	  else \
	    echo "  ✗  expected exit 0, got $$RC"; exit 1; \
	  fi

llvm: test-llvm ## Alias for test-llvm (A0 canned-emission smoke test)
