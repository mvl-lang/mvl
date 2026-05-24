# snake_game — terminal Snake game demonstrating pkg.tui, effects, and game loop.
# Run from this directory or from the repo root.
.PHONY: help guard-mvl build check test-solver test assurance smoke run clean

.DEFAULT_GOAL := help

MVL  := ../../target/debug/mvl
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Ensure MVL binary is built and up-to-date
	@cargo build --manifest-path $(DIR)../../Cargo.toml --quiet

build: guard-mvl ## Compile the example to a native binary (requires bridge.rs)
	$(MVL) build $(DIR)main.mvl

check: guard-mvl ## Type-check all source files
	$(MVL) check $(DIR)game.mvl
	$(MVL) check $(DIR)input.mvl
	$(MVL) check $(DIR)render.mvl
	$(MVL) check $(DIR)main.mvl

test-solver: guard-mvl ## Show per-file solver statistics
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(DIR)game.mvl $(DIR)input.mvl $(DIR)render.mvl $(DIR)main.mvl; do \
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

test: guard-mvl ## Run game logic tests (no terminal required)
	$(MVL) test $(DIR)game_test.mvl

assurance: guard-mvl ## Assurance report (verbose)
	$(MVL) assurance $(DIR) --verbose

smoke: guard-mvl ## Smoke-test: build binary (proves compilation without a terminal)
	$(MVL) build $(DIR)main.mvl

run: guard-mvl ## Run the snake game (requires an interactive terminal)
	$(MVL) run $(DIR)main.mvl

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_snake_game
