# bzip example — MVL Phase 6 demonstration
# Demonstrates: native bit operators (&|^~<<>>), borrow references (&T),
# recursive ADTs (HuffmanTree), pure core + effect boundary.
#
# Run from this directory: make check, make test, make run
.PHONY: help guard-mvl build check test-solver test test-llvm run run-llvm clean

.DEFAULT_GOAL := help

MVL  := ../../target/debug/mvl
DIR  := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

guard-mvl: 
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

test-solver: guard-mvl ## Show per-file solver statistics for all source files
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(DIR)rle.mvl $(DIR)bwt.mvl $(DIR)mtf.mvl $(DIR)huffman.mvl $(DIR)bitstream.mvl; do \
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
