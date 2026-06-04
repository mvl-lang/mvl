# hipaa_healthcare example — HIPAA IFC labels for Protected Health Information
# Run from this directory or from the repo root.
.PHONY: help guard-mvl check lint format test-solver test test-llvm assurance mcdc mutate smoke all clean

.DEFAULT_GOAL := help

MVL  := ../../target/debug/mvl
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Validate MVL binary is present and working (does NOT recompile)
	@$(MVL) --version > /dev/null 2>&1 || { \
		echo ""; \
		echo "  ERROR: MVL compiler not found at: $(MVL)"; \
		echo "  Run 'make build' from the repo root first."; \
		echo ""; \
		exit 1; \
	}

check: guard-mvl ## Type-check all source files
	$(MVL) check $(DIR)ifc.mvl
	$(MVL) check $(DIR)patient.mvl
	$(MVL) check $(DIR)db.mvl

lint: guard-mvl ## Lint all source files
	$(MVL) lint $(DIR)

format: guard-mvl ## Format all .mvl source files
	$(MVL) fmt $(DIR)

test-solver: guard-mvl ## Show per-file solver statistics for all source files
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(DIR)ifc.mvl $(DIR)patient.mvl $(DIR)db.mvl; do \
		name=$$(basename "$$f"); \
		out=$$($(MVL) check "$$f" 2>&1); rc=$$?; \
		if [ $$rc -eq 0 ]; then \
			printf "  $$OK  %s\n" "$$name"; pass=$$((pass + 1)); \
		else \
			printf "  $$FAIL  %s\n" "$$name"; printf "%s\n" "$$out" | sed 's/^/         /'; fail=$$((fail + 1)); \
		fi; \
	done; \
	echo ""; \
	if [ $$fail -eq 0 ]; then \
		printf "  \033[32m✓  $$pass passed, 0 failed\033[0m\n\n"; \
	else \
		printf "  \033[31m✗  $$pass passed, $$fail failed\033[0m\n\n"; exit 1; \
	fi

test: check ## No runtime tests; user-defined relabels not yet in backend (#990)

test-llvm: guard-mvl ## Run tests via LLVM backend
	$(MVL) test $(DIR) --backend=llvm

assurance: guard-mvl ## Assurance report: spec coverage, MC/DC, mutation score (verbose)
	$(MVL) assurance $(DIR) --verbose

mcdc: guard-mvl ## Run MC/DC coverage analysis (ARGS="--masking" to exempt coupled clauses)
	$(MVL) mcdc $(DIR) --verbose $(ARGS)

mutate: guard-mvl ## Run mutation testing
	$(MVL) mutate $(DIR)

all: check test assurance mcdc ## Run full quality pipeline: check + test + assurance + mcdc

smoke: guard-mvl ## Smoke-test: type-check all files
	$(MVL) check $(DIR)ifc.mvl
	$(MVL) check $(DIR)patient.mvl
	$(MVL) check $(DIR)db.mvl

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_hipaa_healthcare
