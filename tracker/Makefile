# MVL Tracker — Makefile
#
# Marimo dashboards for compiler metrics and progress tracking

.PHONY: help build run stop edit edit-stdlib edit-phases clean

PORT ?= 2719

help:
	@echo "MVL Tracker Commands:"
	@echo ""
	@echo "  make build        — Install dependencies (uv sync)"
	@echo "  make run          — Start multi-page dashboard server"
	@echo "  make stop         — Stop the dashboard server"
	@echo "  make edit         — Open all dashboards in Marimo editor"
	@echo "  make edit-stdlib  — Edit stdlib parity dashboard"
	@echo "  make edit-phases  — Edit phase progress dashboard"
	@echo "  make clean        — Remove .venv and caches"
	@echo ""
	@echo "Quick start:"
	@echo "  make build run"
	@echo ""
	@echo "Server runs at: http://localhost:$(PORT)/"

# Install dependencies
build:
	uv sync

# Run multi-page ASGI server (production-like)
run: build
	uv run python serve.py

# Stop server
stop:
	@pkill -f "serve.py" 2>/dev/null || true
	@echo "Stopped tracker server"

# Edit mode (all dashboards)
edit: build
	uv run marimo edit index.py

# Edit specific dashboards
edit-stdlib: build
	uv run marimo edit stdlib_parity.py

edit-phases: build
	uv run marimo edit phase_progress.py

# Clean up
clean:
	rm -rf .venv __pycache__ .marimo __marimo__
	@echo "Cleaned .venv and caches"
