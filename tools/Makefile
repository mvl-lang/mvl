# MVL LSP server — dependency management
# Requires: uv (https://github.com/astral-sh/uv) and an active venv
#
# The tree-sitter grammar lives in https://github.com/mvl-lang/mvl-spec.
# Install the Python binding from a local checkout of that repo:
#     uv pip install /path/to/mvl-spec/tools/tree-sitter

.DEFAULT_GOAL := help

.PHONY: help install run

help:
	@echo "Usage: make <target>"
	@echo ""
	@echo "  install   Install LSP server dependencies into the active venv"
	@echo "  run       Start the MVL LSP server (stdio transport)"
	@echo ""
	@echo "  Note: the tree-sitter grammar binding must be installed separately"
	@echo "  from https://github.com/mvl-lang/mvl-spec (tools/tree-sitter)."

install:
	uv pip install -r requirements.txt

run:
	uv run python lsp_server.py
