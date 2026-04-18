# access_control example — MVL Phase 4 security demonstration
# Run from this directory or from the repo root.
.PHONY: help build check test run clean

MVL  := $(shell test -x ../../target/debug/mvl && echo ../../target/debug/mvl || echo mvl)
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

build: ## Compile the example to a native binary (requires bridge.rs)
	$(MVL) build $(DIR)main.mvl

check: ## Type-check all five source files
	$(MVL) check $(DIR)main.mvl
	$(MVL) check $(DIR)model.mvl
	$(MVL) check $(DIR)auth.mvl
	$(MVL) check $(DIR)rbac.mvl
	$(MVL) check $(DIR)audit.mvl

test: ## Run all standalone _test.mvl files
	$(MVL) test $(DIR)

run: ## Execute the demo scenario (RBAC decisions + auth flow)
	$(MVL) run $(DIR)main.mvl

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_access_control
