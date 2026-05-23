#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Schuberg Philis
"""
MVL Language Server — v1

A minimal LSP server that provides diagnostics for .mvl files by invoking
the MVL compiler (`mvl check --format=json`).

Features: diagnostics on open, change, save.
Non-features (v1): completion, hover, go-to-definition.

Dependencies:
    pip install pygls          # installs lsprotocol as well

Usage (stdio, for editors):
    python tools/lsp_server.py

Override compiler binary:
    MVL_BINARY=/path/to/mvl python tools/lsp_server.py

VS Code extension snippet (settings.json / extension manifest):
    "serverOptions": {
        "command": "python",
        "args": ["${workspaceFolder}/tools/lsp_server.py"]
    },
    "clientOptions": { "documentSelector": [{ "language": "mvl" }] }

Neovim (via nvim-lspconfig custom server):
    require('lspconfig.configs')['mvl'] = {
        default_config = {
            cmd = { 'python', vim.fn.stdpath('data') .. '/mvl/tools/lsp_server.py' },
            filetypes = { 'mvl' },
            root_dir = require('lspconfig.util').root_pattern('Cargo.toml', '.git'),
        },
    }
    require('lspconfig')['mvl'].setup{}
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from lsprotocol import types as lsp
from pygls.server import LanguageServer

SERVER_NAME = "mvl-lsp"
SERVER_VERSION = "0.1.0"

# ── Compiler binary discovery ─────────────────────────────────────────────────


def _find_mvl_binary() -> str:
    """Return path to the mvl compiler, searching in priority order."""
    # 1. Explicit env override
    if env := os.environ.get("MVL_BINARY"):
        return env

    # 2. mvl on PATH
    if found := shutil.which("mvl"):
        return found

    # 3. Local build in this repo (release preferred, then debug)
    repo_root = Path(__file__).parent.parent
    for variant in ("release", "debug"):
        candidate = repo_root / "target" / variant / "mvl"
        if candidate.exists():
            return str(candidate)

    # 4. Fallback — let it fail at runtime with a clear message
    return "mvl"


MVL_BIN = _find_mvl_binary()

# ── Diagnostic parsing ────────────────────────────────────────────────────────

# Stderr parse-error format: "error at {line}:{col}: {message}"
_PARSE_RE = re.compile(r"error at (\d+):(\d+): (.+)")
# Stderr type-check format: "{file}:{line}:{col}: error[req{N}]: {message}"
_CHECK_RE = re.compile(r"[^:]+:(\d+):(\d+): error\[req\d+\]: (.+)")
# Stderr resolver format: "error[resolver]: {message}"
_RESOLVER_RE = re.compile(r"error\[resolver\]: (.+)")


def _range(line: int, col: int, length: int = 1) -> lsp.Range:
    """Build a 0-based LSP Range from 1-based (line, col)."""
    l0 = max(0, line - 1)
    c0 = max(0, col - 1)
    return lsp.Range(
        start=lsp.Position(line=l0, character=c0),
        end=lsp.Position(line=l0, character=c0 + length),
    )


def _diag(
    line: int,
    col: int,
    message: str,
    code: str | None = None,
    severity: lsp.DiagnosticSeverity = lsp.DiagnosticSeverity.Error,
) -> lsp.Diagnostic:
    return lsp.Diagnostic(
        range=_range(line, col),
        message=message,
        severity=severity,
        source=SERVER_NAME,
        code=code,
    )


def _parse_stdout(stdout: str) -> list[lsp.Diagnostic]:
    """Parse JSON output from `mvl check --format=json` (stdout)."""
    if not stdout.strip():
        return []
    try:
        data = json.loads(stdout)
    except json.JSONDecodeError:
        return []

    result: list[lsp.Diagnostic] = []
    for err in data.get("errors", []):
        loc = err.get("location", {})
        result.append(
            _diag(
                line=loc.get("line", 1),
                col=loc.get("column", 1),
                message=err.get("message", "unknown error"),
                code=err.get("code"),
            )
        )
    return result


def _parse_stderr(stderr: str) -> list[lsp.Diagnostic]:
    """Parse human-readable errors from stderr (parse + resolver errors)."""
    result: list[lsp.Diagnostic] = []
    seen: set[tuple[int, int, str]] = set()  # deduplicate

    for line in stderr.splitlines():
        m = _PARSE_RE.match(line) or _CHECK_RE.match(line)
        if m:
            lineno, col, msg = int(m.group(1)), int(m.group(2)), m.group(3).strip()
            key = (lineno, col, msg)
            if key not in seen:
                seen.add(key)
                result.append(_diag(lineno, col, msg))
            continue

        if m := _RESOLVER_RE.match(line):
            msg = m.group(1).strip()
            key = (1, 1, msg)
            if key not in seen:
                seen.add(key)
                result.append(_diag(1, 1, msg))

    return result


# ── Compiler invocation ───────────────────────────────────────────────────────


def _run_check(source: str, file_path: str) -> list[lsp.Diagnostic]:
    """
    Write `source` to a temp file and run `mvl check --format=json` on it.

    The temp file preserves the original file extension so the compiler's
    module loader behaves correctly.
    """
    suffix = Path(file_path).suffix or ".mvl"
    tmp_path: str | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=suffix, delete=False, encoding="utf-8"
        ) as tmp:
            tmp.write(source)
            tmp_path = tmp.name

        result = subprocess.run(
            [MVL_BIN, "check", "--format=json", tmp_path],
            capture_output=True,
            text=True,
            timeout=30,
        )
    except FileNotFoundError:
        return [
            _diag(
                1,
                1,
                f"mvl compiler not found: {MVL_BIN!r}. "
                "Set MVL_BINARY or add mvl to PATH.",
                severity=lsp.DiagnosticSeverity.Warning,
            )
        ]
    except subprocess.TimeoutExpired:
        return [
            _diag(
                1,
                1,
                "mvl check timed out (>30s)",
                severity=lsp.DiagnosticSeverity.Warning,
            )
        ]
    finally:
        if tmp_path:
            try:
                os.unlink(tmp_path)
            except OSError:
                pass

    diagnostics: list[lsp.Diagnostic] = []
    # Parse errors → stderr; type errors → stdout (JSON)
    diagnostics.extend(_parse_stderr(result.stderr))
    diagnostics.extend(_parse_stdout(result.stdout))
    return diagnostics


def _run_check_saved(file_path: str) -> list[lsp.Diagnostic]:
    """Run `mvl check --format=json` directly on a saved file."""
    try:
        result = subprocess.run(
            [MVL_BIN, "check", "--format=json", file_path],
            capture_output=True,
            text=True,
            timeout=30,
        )
    except FileNotFoundError:
        return [
            _diag(
                1,
                1,
                f"mvl compiler not found: {MVL_BIN!r}. "
                "Set MVL_BINARY or add mvl to PATH.",
                severity=lsp.DiagnosticSeverity.Warning,
            )
        ]
    except subprocess.TimeoutExpired:
        return [_diag(1, 1, "mvl check timed out (>30s)", severity=lsp.DiagnosticSeverity.Warning)]

    diagnostics: list[lsp.Diagnostic] = []
    diagnostics.extend(_parse_stderr(result.stderr))
    diagnostics.extend(_parse_stdout(result.stdout))
    return diagnostics


# ── LSP server ────────────────────────────────────────────────────────────────

server = LanguageServer(SERVER_NAME, SERVER_VERSION)


def _uri_to_path(uri: str) -> str:
    from urllib.parse import unquote

    if uri.startswith("file://"):
        return unquote(uri[7:])
    return uri


def _publish(ls: LanguageServer, uri: str, diagnostics: list[lsp.Diagnostic]) -> None:
    ls.publish_diagnostics(uri, diagnostics)


# ── Handlers ──────────────────────────────────────────────────────────────────


@server.feature(lsp.TEXT_DOCUMENT_DID_OPEN)
def did_open(ls: LanguageServer, params: lsp.DidOpenTextDocumentParams) -> None:
    doc = params.text_document
    diags = _run_check(doc.text, _uri_to_path(doc.uri))
    _publish(ls, doc.uri, diags)


@server.feature(lsp.TEXT_DOCUMENT_DID_CHANGE)
def did_change(ls: LanguageServer, params: lsp.DidChangeTextDocumentParams) -> None:
    if not params.content_changes:
        return
    # Full sync: last event contains the complete current content
    last = params.content_changes[-1]
    text = last.text if hasattr(last, "text") else ""
    if not text:
        return
    doc = params.text_document
    diags = _run_check(text, _uri_to_path(doc.uri))
    _publish(ls, doc.uri, diags)


@server.feature(lsp.TEXT_DOCUMENT_DID_SAVE)
def did_save(ls: LanguageServer, params: lsp.DidSaveTextDocumentParams) -> None:
    doc = params.text_document
    path = _uri_to_path(doc.uri)
    diags = _run_check_saved(path)
    _publish(ls, doc.uri, diags)


@server.feature(lsp.TEXT_DOCUMENT_DID_CLOSE)
def did_close(ls: LanguageServer, params: lsp.DidCloseTextDocumentParams) -> None:
    # Clear diagnostics when the file is closed
    _publish(ls, params.text_document.uri, [])


# ── Entry point ───────────────────────────────────────────────────────────────

if __name__ == "__main__":
    print(f"Starting {SERVER_NAME} v{SERVER_VERSION}", file=sys.stderr)
    print(f"Using mvl binary: {MVL_BIN}", file=sys.stderr)
    server.start_io()
