# examples/crud_api — full CRUD REST API over SQLite using pkg.http REST helpers.
#
# Uses both pkg.http (routing, REST helpers) and pkg.sqlite (database).
# Commands run from REPO_ROOT so both packages resolve without symlinks.
#
# Run:   make run     → start server on 127.0.0.1:8080
# Test:  make check   → type-check all source files
.PHONY: help guard-mvl check test-solver test smoke run clean

.DEFAULT_GOAL := help

MVL       := ../../target/debug/mvl
DIR       := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
REPO_ROOT := $(abspath $(DIR)../..)

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl:
	@$(MVL) --version > /dev/null 2>&1 || { echo "Run 'make build' from repo root first."; exit 1; }

check: guard-mvl ## Type-check all source files
	cd $(REPO_ROOT) && $(abspath $(MVL)) check $(DIR)models.mvl
	cd $(REPO_ROOT) && $(abspath $(MVL)) check $(DIR)db.mvl
	cd $(REPO_ROOT) && $(abspath $(MVL)) check $(DIR)handlers.mvl
	cd $(REPO_ROOT) && $(abspath $(MVL)) check $(DIR)main.mvl

test-solver: guard-mvl ## Show per-file solver statistics
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(DIR)models.mvl $(DIR)db.mvl $(DIR)handlers.mvl $(DIR)main.mvl; do \
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

test: guard-mvl ## Run unit tests (check all files — no _test.mvl yet)
	cd $(REPO_ROOT) && $(abspath $(MVL)) test $(DIR)

smoke: guard-mvl ## Smoke-test: build binary (proves compilation without a terminal)
	cd $(REPO_ROOT) && $(abspath $(MVL)) build $(DIR)main.mvl

run: guard-mvl ## Start the CRUD API server on 127.0.0.1:8080 (Ctrl-C to stop)
	cd $(REPO_ROOT) && $(abspath $(MVL)) run $(DIR)main.mvl

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_crud_api
