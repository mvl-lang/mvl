# examples/sqlite_basic — SQLite basic example Makefile
.PHONY: help guard-mvl run check test assurance clean

.DEFAULT_GOAL := help

MVL      := ../../target/debug/mvl
DIR      := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
REPO_ROOT := $(abspath $(DIR)../..)

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl:
	@$(MVL) --version > /dev/null 2>&1 || { echo "Run 'make build' from repo root first."; exit 1; }

check: guard-mvl ## Type-check example files (main.mvl + main_test.mvl)
	cd $(REPO_ROOT) && $(abspath $(MVL)) check $(DIR)

test: guard-mvl ## Run unit tests from main_test.mvl
	cd $(REPO_ROOT) && $(abspath $(MVL)) test $(DIR)

assurance: guard-mvl ## Full assurance: check + unit tests + assurance report
	cd $(REPO_ROOT) && $(abspath $(MVL)) check $(DIR)
	cd $(REPO_ROOT) && $(abspath $(MVL)) test $(DIR)
	cd $(REPO_ROOT) && $(abspath $(MVL)) assurance $(DIR) --verbose

run: guard-mvl ## Run the example (runs from repo root so pkg.* resolution finds .mvl/pkg/sqlite)
	cd $(REPO_ROOT) && $(abspath $(MVL)) run $(DIR)main.mvl

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_sqlite_basic
