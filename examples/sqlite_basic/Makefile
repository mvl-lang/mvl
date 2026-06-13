# examples/sqlite_basic — SQLite basic example Makefile
.PHONY: help guard-mvl smoke run check prove test-solver test assurance clean

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

prove: guard-mvl ## Prove per-call-site refinement proof layer breakdown
	cd $(REPO_ROOT) && $(abspath $(MVL)) prove $(DIR)

test-solver: guard-mvl ## Show per-file solver statistics
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(DIR)main.mvl; do \
		name=$$(basename "$$f"); \
		out=$$(cd $(REPO_ROOT) && $(abspath $(MVL)) check "$$f" 2>&1); rc=$$?; \
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

test: guard-mvl ## Run unit tests from main_test.mvl
	cd $(REPO_ROOT) && $(abspath $(MVL)) test $(DIR)

assurance: guard-mvl ## Full assurance: check + unit tests + assurance report
	cd $(REPO_ROOT) && $(abspath $(MVL)) check $(DIR)
	cd $(REPO_ROOT) && $(abspath $(MVL)) test $(DIR)
	cd $(REPO_ROOT) && $(abspath $(MVL)) assurance $(DIR) --verbose

smoke: guard-mvl ## Smoke-test: build binary (proves compilation without a terminal)
	cd $(REPO_ROOT) && $(abspath $(MVL)) build $(DIR)main.mvl

run: guard-mvl ## Run the example
	cd $(REPO_ROOT) && $(abspath $(MVL)) run $(DIR)main.mvl

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_sqlite_basic
