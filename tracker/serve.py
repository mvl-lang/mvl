"""ASGI server for MVL Tracker — multi-page Marimo app.

Uses Marimo's create_asgi_app() to mount each dashboard at a clean path.

Run with: make run  (or: uv run python serve.py)
"""

from pathlib import Path

import uvicorn
from marimo import create_asgi_app

APP_DIR = Path(__file__).parent
PORT = 2719

# Build multi-page app
marimo_app = (
    create_asgi_app()
    .with_app(path="/", root=str(APP_DIR / "index.py"))
    .with_app(path="/stdlib", root=str(APP_DIR / "stdlib_parity.py"))
    .with_app(path="/phases", root=str(APP_DIR / "phase_progress.py"))
).build()

if __name__ == "__main__":
    print()
    print("  MVL Tracker — Compiler Metrics Dashboard")
    print("  =========================================")
    print(f"  http://localhost:{PORT}/        — Launcher")
    print(f"  http://localhost:{PORT}/stdlib  — Stdlib Parity (Rust vs LLVM)")
    print(f"  http://localhost:{PORT}/phases  — Nine Phases Progress")
    print()
    uvicorn.run(marimo_app, host="0.0.0.0", port=PORT)
