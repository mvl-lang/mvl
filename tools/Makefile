# MVL LSP server — dependency management
# Requires: uv (https://github.com/astral-sh/uv) and an active venv
#
# The tree-sitter grammar is vendored via the mvl-spec submodule.
# Install the Python binding after `git submodule update --init`:
#     uv pip install ../vendor/mvl-spec/tools/tree-sitter

.DEFAULT_GOAL := help

.PHONY: help install run

help:
	@echo "Usage: make <target>"
	@echo ""
	@echo "  install   Install LSP server dependencies into the active venv"
	@echo "  run       Start the MVL LSP server (stdio transport)"
	@echo ""
	@echo "  Note: the tree-sitter grammar binding must be installed separately"
	@echo "  from the vendored mvl-spec submodule (../vendor/mvl-spec/tools/tree-sitter)."

install:
	uv pip install -r requirements.txt

run:
	uv run python lsp_server.py
