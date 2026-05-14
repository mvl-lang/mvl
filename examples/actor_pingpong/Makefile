# actor_pingpong — basic actor-to-actor communication (issue #580)
#
# Demonstrates Phase 8 actor model: actor keyword, pub fn behaviors,
# val/tag capabilities, concurrently { } structured concurrency.
#
# Phase 8 feature — requires actor runtime (#695, #696).
# `make test` passes today by reporting pending status; `make run` requires
# a Phase 8 compiler build.
#
# Run from this directory or from the repo root.
.PHONY: help guard-mvl check test run clean

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

check: guard-mvl ## Type-check main.mvl (requires Phase 8 actor support)
	$(MVL) check $(DIR)main.mvl

test: ## Report Phase 8 pending status (actor runtime not yet implemented)
	@echo ""
	@echo "  actor_pingpong: Phase 8 example -- actor runtime pending (#695, #696)"
	@echo "  Syntax design complete; run \`make run\` once actor runtime lands."
	@echo ""

run: guard-mvl ## Run the ping-pong exchange (requires Phase 8 actor runtime)
	$(MVL) run $(DIR)main.mvl

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_actor_pingpong
