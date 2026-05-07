"""ASGI server for MVL Tracker — multi-page Marimo app.

Uses Marimo's create_asgi_app() to mount each dashboard at a clean path.

Run with: make run  (or: uv run python serve.py)
"""

from pathlib import Path

import uvicorn
from marimo import create_asgi_app

APP_DIR = Path(__file__).parent
PORT = 2719

# Home button: injected on all pages except /
# Uses Unicode house character (⌂ = \u2302)
_HOME_BUTTON_STYLE = """
<style>
#mvl-home-button {
    position: fixed;
    top: 8px;
    left: 8px;
    z-index: 2147483647;
    width: 28px;
    height: 28px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: #3b82f6;
    font-size: 18px;
    line-height: 1;
    text-decoration: none;
    border-radius: 4px;
    transition: color 0.15s ease, background-color 0.15s ease;
}
#mvl-home-button:hover,
#mvl-home-button:focus {
    color: #1d4ed8;
    background: rgba(59, 130, 246, 0.1);
    outline: none;
}
</style>
"""

_HOME_BUTTON_SCRIPT = """
<script>
(function () {
    if (window.location.pathname === "/") return;
    function makeButton() {
        var a = document.createElement("a");
        a.id = "mvl-home-button";
        a.href = "/";
        a.title = "Home";
        a.setAttribute("aria-label", "Home");
        a.innerHTML = '<span aria-hidden="true">\\u2302</span>';
        return a;
    }
    function ensure() {
        if (!document.body) return;
        if (!document.getElementById("mvl-home-button")) {
            document.body.appendChild(makeButton());
        }
    }
    if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", ensure);
    } else {
        ensure();
    }
    var observer = new MutationObserver(ensure);
    function startObserver() {
        if (document.body) observer.observe(document.body, {childList: true});
        else setTimeout(startObserver, 50);
    }
    startObserver();
})();
</script>
"""

html_head = _HOME_BUTTON_STYLE + _HOME_BUTTON_SCRIPT

# Build multi-page app
marimo_app = (
    create_asgi_app(html_head=html_head)
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
