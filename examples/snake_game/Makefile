# snake_game example — MVL Phase 2 effects + purity demonstration
# Demonstrates: ! Terminal, ! Clock, ! Random effects; pure game logic.
# Run from this directory or from the repo root.
.PHONY: help build check test run clean

MVL  := $(shell test -x ../../target/debug/mvl && echo ../../target/debug/mvl || echo mvl)
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

build: ## Compile the example to a native binary (requires bridge.rs + crossterm)
	$(MVL) build $(DIR)main.mvl

check: ## Type-check all source files
	$(MVL) check $(DIR)main.mvl
	$(MVL) check $(DIR)game.mvl
	$(MVL) check $(DIR)input.mvl
	$(MVL) check $(DIR)render.mvl

test: ## Run all standalone *_test.mvl files (no terminal needed)
	$(MVL) test $(DIR)

run: ## Launch the snake game
	$(MVL) run $(DIR)main.mvl

clean: ## Remove build artefacts
	$(MVL) clean $(DIR)
