# snake_game — terminal Snake game demonstrating pkg.tui, effects, and game loop.
# Run from this directory or from the repo root.
.PHONY: help guard-mvl build check test assurance run clean

.DEFAULT_GOAL := help

MVL  := ../../target/debug/mvl
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Validate MVL binary is present and working
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
	$(MVL) check $(DIR)game.mvl
	$(MVL) check $(DIR)input.mvl
	$(MVL) check $(DIR)render.mvl
	$(MVL) check $(DIR)main.mvl

test: guard-mvl ## Run game logic tests (no terminal required)
	$(MVL) test $(DIR)game_test.mvl

assurance: check test ## Type-check all sources and run all tests

run: guard-mvl ## Run the snake game (requires an interactive terminal)
	$(MVL) run $(DIR)main.mvl

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_snake_game
