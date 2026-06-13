# examples/crud_api — full CRUD REST API over SQLite using pkg.http REST helpers.
#
# Uses both pkg.http (routing, REST helpers) and pkg.sqlite (database).
# Commands run from REPO_ROOT so both packages resolve without symlinks.
#
# Run:   make run        → start server on 127.0.0.1:8080
# Test:  make check      → type-check all source files
# Logs:  make assurance  → assurance report (all source files)
.PHONY: help guard-mvl lint check prove test-solver test test-llvm assurance smoke run clean

.DEFAULT_GOAL := help

MVL       := ../../target/debug/mvl
DIR       := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
REPO_ROOT := $(abspath $(DIR)../..)

SRCS := $(DIR)models.mvl $(DIR)config.mvl $(DIR)db.mvl $(DIR)seed.mvl $(DIR)handlers.mvl $(DIR)main.mvl

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl:
	@$(MVL) --version > /dev/null 2>&1 || { echo "Run 'make build' from repo root first."; exit 1; }

lint: guard-mvl ## Lint all source files
	@for f in $(SRCS); do \
		cd $(REPO_ROOT) && $(abspath $(MVL)) lint "$$f"; \
	done

check: guard-mvl ## Type-check all source files
	@for f in $(SRCS); do \
		cd $(REPO_ROOT) && $(abspath $(MVL)) check "$$f"; \
	done

prove: guard-mvl ## Prove per-call-site refinement proof layer breakdown
	cd $(REPO_ROOT) && $(abspath $(MVL)) prove $(DIR)

test-solver: guard-mvl ## Show per-file solver statistics
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(SRCS); do \
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

test: guard-mvl ## Run unit tests (config_test.mvl, main_test.mvl)
	cd $(REPO_ROOT) && $(abspath $(MVL)) test $(DIR)

test-llvm: guard-mvl ## Run tests via LLVM backend
	cd $(REPO_ROOT) && $(abspath $(MVL)) test $(DIR) --backend=llvm

assurance: guard-mvl ## Assurance report (all source files)
	cd $(REPO_ROOT) && $(abspath $(MVL)) assurance $(DIR)

smoke: guard-mvl ## Smoke-test: build binary (proves compilation without a terminal)
	cd $(REPO_ROOT) && $(abspath $(MVL)) build $(DIR)main.mvl

run: guard-mvl ## Start the CRUD API server on 127.0.0.1:8080 (Ctrl-C to stop)
	cd $(REPO_ROOT) && $(abspath $(MVL)) run $(DIR)main.mvl

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_crud_api
