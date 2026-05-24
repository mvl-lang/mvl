# MVL LSP server — dependency management
# Requires: uv (https://github.com/astral-sh/uv) and an active venv

REPO_ROOT := $(abspath ..)
GRAMMAR   := $(REPO_ROOT)/etc/tree-sitter-mvl

.DEFAULT_GOAL := help

.PHONY: help install run

help:
	@echo "Usage: make <target>"
	@echo ""
	@echo "  install   Install LSP server dependencies into the active venv"
	@echo "  run       Start the MVL LSP server (stdio transport)"

install:
	uv pip install -r requirements.txt
	uv pip install "$(GRAMMAR)"

run:
	uv run python lsp_server.py
