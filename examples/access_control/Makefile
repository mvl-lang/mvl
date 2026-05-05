# access_control example — MVL Phase 4 security demonstration
# Run from this directory or from the repo root.
.PHONY: help guard-mvl build check test test-llvm run run-llvm clean

MVL  := $(shell test -x ../../target/debug/mvl && echo ../../target/debug/mvl || echo mvl)
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

check: guard-mvl ## Type-check all five source files
	$(MVL) check $(DIR)main.mvl
	$(MVL) check $(DIR)model.mvl
	$(MVL) check $(DIR)auth.mvl
	$(MVL) check $(DIR)rbac.mvl
	$(MVL) check $(DIR)audit.mvl

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
