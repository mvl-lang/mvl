# MVL LSP server — dependency management
# Requires: uv  (https://github.com/astral-sh/uv)

VENV      := .venv
PYTHON    := $(VENV)/bin/python
REPO_ROOT := $(abspath ..)
GRAMMAR   := $(REPO_ROOT)/etc/tree-sitter-mvl

.DEFAULT_GOAL := help

.PHONY: help install run

help:
	@echo "Usage: make <target>"
	@echo ""
	@echo "  install   Create .venv and install LSP server dependencies"
	@echo "  run       Start the MVL LSP server (stdio transport)"

install:
	uv venv $(VENV)
	uv pip install --python $(PYTHON) -r requirements.txt
	uv pip install --python $(PYTHON) "$(GRAMMAR)"
	@echo ""
	@echo "Done. Run with: make run"

run:
	$(PYTHON) lsp_server.py
