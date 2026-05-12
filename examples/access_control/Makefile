# access_control example — MVL Phase 4 security demonstration
# Run from this directory or from the repo root.
.PHONY: help guard-mvl build check test-solver test test-llvm run run-llvm clean

.DEFAULT_GOAL := help

MVL  := ../../target/debug/mvl
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Validate MVL binary is present and working (does NOT recompile)
	@$(MVL) --version > /dev/null 2>&1 || { \
		echo ""; \
		echo "  ERROR: MVL compiler not found at: $(MVL)"; \
		echo "  Run 'make build' from the repo root first."; \
		echo ""; \
		exit 1; \
	}

build: guard-mvl ## Compile the example to a native binary (requires bridge.rs)
	$(MVL) build $(DIR)main.mvl

check: guard-mvl ## Type-check all source files
	$(MVL) check $(DIR)model.mvl
	$(MVL) check $(DIR)auth.mvl
	$(MVL) check $(DIR)rbac.mvl
	$(MVL) check $(DIR)audit.mvl
	$(MVL) check $(DIR)main.mvl

test-solver: guard-mvl ## Show per-file solver statistics for all source files
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(DIR)model.mvl $(DIR)auth.mvl $(DIR)rbac.mvl $(DIR)audit.mvl $(DIR)main.mvl; do \
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

test: guard-mvl ## Run all standalone _test.mvl files
	$(MVL) test $(DIR)

test-llvm: guard-mvl ## Run tests via LLVM backend
	$(MVL) test $(DIR) --backend=llvm

run: guard-mvl ## Execute the demo scenario (RBAC decisions + auth flow)
	$(MVL) run $(DIR)main.mvl

run-llvm: guard-mvl ## Execute via LLVM backend
	$(MVL) run $(DIR)main.mvl --backend=llvm

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_access_control
