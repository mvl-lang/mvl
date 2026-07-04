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

# LLVM path (issue #1118).  MVL-hosted backend under compiler/backends/llvm/.
# A0: canned exit-0 emission (empty.mvl).
# A1a: integer literals + arithmetic; stdout carries the printed result.
EMIT_LL := $(ROOT)/compiler/backends/llvm/backend_llvm.mvl
SRC_LL  := $(SPIKE)/empty.mvl
TIR_LL  := /tmp/spike004_tir_llvm.json
LL      := /tmp/spike004_out.ll
ASM_LL  := /tmp/spike004_out.s
BIN_LL  := /tmp/spike004_bin_llvm

# A1a+A1b+A1c+A1d cases: "src.mvl:expected_stdout".
LL_CASES := empty.mvl:0 two_plus_three.mvl:5 six_times_seven.mvl:42 nested_arith.mvl:20 \
            add_call.mvl:5 let_bind.mvl:10 multi_stmt.mvl:30 \
            cmp_lt.mvl:1 if_expr.mvl:7 bool_lit.mvl:42 max.mvl:9 if_eq.mvl:100 \
            ref_mut.mvl:42 while_counter.mvl:45 factorial.mvl:120 while_if.mvl:20 \
            else_if.mvl:20 match_int.mvl:20 match_bool.mvl:7 match_expr.mvl:300 \
            match_bind.mvl:43 match_scope.mvl:15 \
            match_option_some.mvl:43 match_option_none.mvl:99 match_option_flow.mvl:100 \
            match_result_ok.mvl:7 match_result_err.mvl:42 \
            fn_ret_option.mvl:21 fn_ret_result.mvl:100 \
            for_sum.mvl:45 for_count.mvl:5 \
            enum_unit.mvl:2 enum_direction.mvl:1 \
            list_sum.mvl:15 list_filter.mvl:12 \
            str_hello.mvl:hello str_greet.mvl:greetings str_two.mvl:alpha

.PHONY: help build guard-mvl tir emit compile run test check all clean \
        tir-llvm emit-llvm compile-llvm run-llvm test-llvm llvm test-llvm-arith

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

test-llvm: compile-llvm ## Full LLVM pipeline test — check stdout matches expected
	@ACTUAL=$$($(BIN_LL)); RC=$$?; \
	  if [ "$$RC" != "0" ]; then \
	    echo "  ✗  non-zero exit: $$RC"; exit 1; \
	  fi; \
	  echo "  stdout: $$ACTUAL"

test-llvm-arith: guard-mvl ## Run all A1a arithmetic cases; assert stdout per case
	@set -e; \
	for case in $(LL_CASES); do \
	  src="$${case%%:*}"; expected="$${case##*:}"; \
	  echo "→ $$src (expected: $$expected)"; \
	  cd "$(ROOT)" && $(MVL) tir $(SPIKE)/$$src > $(TIR_LL); \
	  $(MVL) run $(EMIT_LL) < $(TIR_LL) 2>/dev/null \
	    | grep -v "^Transpiled to:\|^Running: cargo" > $(LL); \
	  llc $(LL) -o $(ASM_LL); \
	  cc $(ASM_LL) -o $(BIN_LL); \
	  actual=$$($(BIN_LL)); \
	  if [ "$$actual" = "$$expected" ]; then \
	    echo "  ✓  $$actual"; \
	  else \
	    echo "  ✗  expected '$$expected', got '$$actual'"; exit 1; \
	  fi; \
	done

llvm: test-llvm-arith ## A1a arithmetic corpus (defaults to arith cases)
