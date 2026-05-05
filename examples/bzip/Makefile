# bzip example — MVL Phase 6 demonstration
# Demonstrates: native bit operators (&|^~<<>>), borrow references (&T),
# recursive ADTs (HuffmanTree), pure core + effect boundary.
#
# Run from this directory: make check, make test, make run
.PHONY: help guard-mvl build check test test-llvm run run-llvm clean

MVL  := $(shell test -x ../../target/debug/mvl && echo ../../target/debug/mvl || echo mvl)
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Validate MVL binary is present and working
	@$(MVL) --version > /dev/null 2>&1 || { \
		echo ""; \
		echo "  ERROR: MVL compiler not found at: $(MVL)"; \
		echo "  Run 'make build' from the repo root first."; \
		echo ""; \
		exit 1; \
	}

build: guard-mvl ## Compile the example to a native binary
	$(MVL) build $(DIR)main.mvl

check: guard-mvl ## Type-check self-contained source modules
	$(MVL) check $(DIR)rle.mvl
	$(MVL) check $(DIR)bwt.mvl
	$(MVL) check $(DIR)mtf.mvl
	$(MVL) check $(DIR)huffman.mvl
	$(MVL) check $(DIR)bitstream.mvl

test: guard-mvl ## Run all inline test fn and roundtrip_test.mvl (transpiler backend)
	$(MVL) test $(DIR)

test-llvm: guard-mvl ## Run tests via LLVM backend
	$(MVL) test $(DIR) --backend=llvm

run: guard-mvl ## Run the demo (compress a hardcoded byte sequence, print result)
	$(MVL) run $(DIR)main.mvl

run-llvm: guard-mvl ## Run the demo via LLVM backend
	$(MVL) run $(DIR)main.mvl --backend=llvm

# Show the effect boundary — should print only main.mvl lines
effects: ## Grep effect annotations — pure core has none
	@echo "Functions with effects (should be main() only):"
	@grep -n '!' $(DIR)*.mvl | grep -v '^$(DIR)main.mvl' | grep -v '//' || echo "  (none — pure core confirmed)"
	@grep -n '!' $(DIR)main.mvl | grep -v '//'

clean: ## Remove build artifacts
	rm -rf $(TMPDIR)mvl_build_bzip
