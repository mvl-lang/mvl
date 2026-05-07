.PHONY: build serve clean

# Install dependencies and create .venv
build:
	uv sync

# Serve all dashboards (opens browser)
serve: build
	uv run marimo edit .

# Serve specific dashboard
serve-stdlib: build
	uv run marimo edit stdlib_parity.py

serve-phases: build
	uv run marimo edit phase_progress.py

# Clean up
clean:
	rm -rf .venv __pycache__ .marimo
