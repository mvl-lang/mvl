# log_to_file example — structured logging with IFC and refinement types.
# Run from this directory: make check, make test, etc.
.PHONY: help guard-mvl lint check test-solver test assurance smoke run clean

.DEFAULT_GOAL := help

MVL       := ../../target/debug/mvl
DIR       := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
REPO_ROOT := $(abspath $(DIR)../..)

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl:
	@$(MVL) --version > /dev/null 2>&1 || { \
		echo ""; \
		echo "  ERROR: MVL compiler not found at: $(MVL)"; \
		echo "  Run 'cargo build' from the repo root first."; \
		echo ""; \
		exit 1; \
	}

lint: guard-mvl ## Lint main.mvl
	cd $(REPO_ROOT) && $(abspath $(MVL)) lint $(DIR)main.mvl

check: guard-mvl ## Type-check all files in this directory
	cd $(REPO_ROOT) && $(abspath $(MVL)) check $(DIR)

test-solver: guard-mvl ## Show solver statistics for main.mvl
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

test: guard-mvl ## Run all unit tests in main_test.mvl (4 tests)
	cd $(REPO_ROOT) && $(abspath $(MVL)) test $(DIR)

assurance: guard-mvl ## Full assurance: check + unit tests + assurance report
	cd $(REPO_ROOT) && $(abspath $(MVL)) check $(DIR)
	cd $(REPO_ROOT) && $(abspath $(MVL)) test $(DIR)
	cd $(REPO_ROOT) && $(abspath $(MVL)) assurance $(DIR) --verbose

smoke: guard-mvl ## Smoke-test: build binary (proves compilation without a terminal)
	cd $(REPO_ROOT) && $(abspath $(MVL)) build $(DIR)main.mvl

run: guard-mvl ## Run the example; writes $(DIR)app.log and prints it
	cd $(DIR) && MVL_LOG_FILE=$(DIR)app.log $(abspath $(MVL)) run $(DIR)main.mvl
	@echo ""
	@echo "── app.log ─────────────────────────────────────────────"
	@cat $(DIR)app.log
	@echo "────────────────────────────────────────────────────────"

clean: ## Remove build artifacts and app.log
	rm -rf $(TMPDIR)mvl_build_log_to_file
	rm -f $(DIR)app.log
