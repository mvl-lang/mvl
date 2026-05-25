# anthropic_chat — Anthropic Claude API example
#
# Demonstrates pkg.anthropic SDK with IFC-aware Secret/Tainted types.
#
# Package resolution: .mvl/pkg/* symlinks to ../../pkg/* so that
# mvl commands run from this directory can find the packages.
.PHONY: help guard-mvl check clean

.DEFAULT_GOAL := help

MVL       := ../../target/debug/mvl
DIR       := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
REPO_ROOT := $(abspath $(DIR)../..)

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Ensure MVL binary is built and up-to-date
	@cargo build --manifest-path $(DIR)../../Cargo.toml --quiet

.mvl/pkg/anthropic:
	@mkdir -p .mvl/pkg
	@ln -sfn $(REPO_ROOT)/pkg/anthropic .mvl/pkg/anthropic

.mvl/pkg/tls:
	@mkdir -p .mvl/pkg
	@ln -sfn $(REPO_ROOT)/pkg/tls .mvl/pkg/tls

check: guard-mvl .mvl/pkg/anthropic .mvl/pkg/tls ## Type-check main.mvl
	$(MVL) check $(DIR)main.mvl

test: check ## No unit tests; delegates to type-check

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_anthropic_chat .mvl
