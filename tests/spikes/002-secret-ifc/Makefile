# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Schuberg Philis
#
# Spike 002 — Secret[String] IFC enforcement for API keys  (issue #872)
#
# Validates that Secret[T] values loaded from environment cannot leak
# to public sinks (println, eprintln, log, file write). The IFC lattice
# (Public < Clean < Tainted < Secret) should reject downward flows at
# compile time; declassify() is the only auditable exit.
#
# Run from repo root:
#   make -C tests/spikes/002-secret-ifc
# Or from this directory:
#   make

ROOT := $(shell git -C "$(dir $(abspath $(lastword $(MAKEFILE_LIST))))" rev-parse --show-toplevel)
MVL  := $(ROOT)/target/debug/mvl

.PHONY: help build guard-mvl check test test-positive test-negative

.DEFAULT_GOAL := help

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-16s\033[0m %s\n", $$1, $$2}'

guard-mvl:
	@$(MVL) --version > /dev/null 2>&1 || { \
		echo "  ERROR: MVL compiler not found at: $(MVL)"; \
		echo "  Run 'cargo build' from the repo root first."; \
		exit 1; \
	}

build: ## Build the MVL compiler
	cd "$(ROOT)" && cargo build --quiet

check: guard-mvl ## Type-check all positive .mvl files (should pass)
	@echo "=== Positive flows (should compile) ==="
	@cd "$(ROOT)" && $(MVL) check tests/spikes/002-secret-ifc/01_positive_flows.mvl && echo "  01_positive_flows: OK" || echo "  01_positive_flows: FAIL"
	@cd "$(ROOT)" && $(MVL) check tests/spikes/002-secret-ifc/03_struct_flows.mvl && echo "  03_struct_flows: OK" || echo "  03_struct_flows: FAIL"
	@cd "$(ROOT)" && $(MVL) check tests/spikes/002-secret-ifc/05_declassify_exit.mvl && echo "  05_declassify_exit: OK" || echo "  05_declassify_exit: FAIL"

test-positive: guard-mvl ## Run positive tests (expect: compile success)
	@echo "=== Positive: must compile ==="
	@cd "$(ROOT)" && $(MVL) check tests/spikes/002-secret-ifc/01_positive_flows.mvl && echo "  PASS: 01_positive_flows" || echo "  FAIL: 01_positive_flows"
	@cd "$(ROOT)" && $(MVL) check tests/spikes/002-secret-ifc/03_struct_flows.mvl && echo "  PASS: 03_struct_flows" || echo "  FAIL: 03_struct_flows"
	@cd "$(ROOT)" && $(MVL) check tests/spikes/002-secret-ifc/05_declassify_exit.mvl && echo "  PASS: 05_declassify_exit" || echo "  FAIL: 05_declassify_exit"
	@cd "$(ROOT)" && $(MVL) check tests/spikes/002-secret-ifc/09_pure_mvl_get_secret.mvl && echo "  PASS: 09_pure_mvl_get_secret" || echo "  FAIL: 09_pure_mvl_get_secret"

test-negative: guard-mvl ## Run negative tests (expect: compile FAILURE = IFC rejection)
	@echo "=== Negative: must be REJECTED ==="
	@cd "$(ROOT)" && $(MVL) check tests/spikes/002-secret-ifc/02_leak_println.mvl 2>/dev/null && echo "  FAIL: 02_leak_println compiled (should reject)" || echo "  PASS: 02_leak_println rejected"
	@cd "$(ROOT)" && $(MVL) check tests/spikes/002-secret-ifc/04_leak_return.mvl 2>/dev/null && echo "  FAIL: 04_leak_return compiled (should reject)" || echo "  PASS: 04_leak_return rejected"
	@cd "$(ROOT)" && $(MVL) check tests/spikes/002-secret-ifc/06_leak_struct_field.mvl 2>/dev/null && echo "  FAIL: 06_leak_struct_field compiled (should reject)" || echo "  PASS: 06_leak_struct_field rejected"
	@cd "$(ROOT)" && $(MVL) check tests/spikes/002-secret-ifc/07_leak_format.mvl 2>/dev/null && echo "  FAIL: 07_leak_format compiled (should reject)" || echo "  PASS: 07_leak_format rejected"
	@cd "$(ROOT)" && $(MVL) check tests/spikes/002-secret-ifc/08_leak_log.mvl 2>/dev/null && echo "  FAIL: 08_leak_log compiled (should reject)" || echo "  PASS: 08_leak_log rejected"

test: test-positive test-negative ## Run all spike tests
	@echo "=== Done ==="
