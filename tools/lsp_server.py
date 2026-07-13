#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Schuberg Philis
"""
MVL Language Server — Phase 1 (tree-sitter, no compiler required)

Provides syntax-error diagnostics for .mvl files using the MVL tree-sitter
grammar, which lives in https://github.com/mvl-lang/mvl-spec
(tools/tree-sitter/). No MVL compiler binary needed.

For full type/effect/refinement diagnostics, see the Phase 2 LSP (#1003).

Dependencies:
    pip install pygls tree-sitter
    # Then install the tree-sitter binding from the vendored mvl-spec submodule:
    git submodule update --init --recursive
    pip install ./vendor/mvl-spec/tools/tree-sitter

Run (stdio, for editors):
    python tools/lsp_server.py

Editor extensions (VS Code, Neovim, Zed) live in
https://github.com/mvl-lang/mvl-spec (editors/).
"""

from __future__ import annotations

import sys
from pathlib import Path

from lsprotocol import types as lsp
from pygls.server import LanguageServer

SERVER_NAME = "mvl-lsp"
SERVER_VERSION = "0.1.0"

# ── Tree-sitter parser setup ──────────────────────────────────────────────────

def _load_parser():
    """
    Load the MVL tree-sitter parser from the installed tree_sitter_mvl
    Python binding (built and published from https://github.com/mvl-lang/mvl-spec).

    Returns a configured Parser, or None if tree-sitter is unavailable.
    """
    try:
        from tree_sitter import Language, Parser as TSParser
    except ImportError:
        return None

    try:
        import tree_sitter_mvl as _ts_mvl  # type: ignore[import]
        lang = Language(_ts_mvl.language())
        try:
            return TSParser(lang)          # tree-sitter >= 0.22
        except TypeError:
            p = TSParser()
            p.set_language(lang)           # tree-sitter 0.21
            return p
    except (ImportError, AttributeError):
        return None


_PARSER = _load_parser()

if _PARSER is None:
    print(
        "[mvl-lsp] WARNING: tree-sitter not available — no diagnostics will be reported.\n"
        "  Install: pip install tree-sitter && \\\n"
        "           pip install ./vendor/mvl-spec/tools/tree-sitter",
        file=sys.stderr,
    )

# ── Diagnostic extraction ─────────────────────────────────────────────────────

def _walk(node) -> list[lsp.Diagnostic]:
    """
    Recursively walk a tree-sitter node, collecting ERROR and MISSING nodes
    as LSP Diagnostics.

    Tree-sitter is error-tolerant: it continues parsing after errors and inserts
    ERROR nodes (for unexpected tokens) and MISSING nodes (for absent expected
    tokens). We surface both as syntax errors.
    """
    diagnostics: list[lsp.Diagnostic] = []

    if node.is_missing:
        # MISSING: a required token was absent at this position.
        row, col = node.start_point
        diagnostics.append(lsp.Diagnostic(
            range=lsp.Range(
                start=lsp.Position(line=row, character=col),
                end=lsp.Position(line=row, character=col + 1),
            ),
            message=f"missing {node.type}",
            severity=lsp.DiagnosticSeverity.Error,
            source=SERVER_NAME,
            code="EPARSE",
        ))
    elif node.type == "ERROR":
        # ERROR: the parser could not match the token(s) here.
        start_row, start_col = node.start_point
        end_row, end_col = node.end_point
        # Avoid zero-width ranges
        if start_row == end_row and start_col == end_col:
            end_col += 1
        diagnostics.append(lsp.Diagnostic(
            range=lsp.Range(
                start=lsp.Position(line=start_row, character=start_col),
                end=lsp.Position(line=end_row, character=end_col),
            ),
            message="syntax error",
            severity=lsp.DiagnosticSeverity.Error,
            source=SERVER_NAME,
            code="EPARSE",
        ))
        # Don't recurse into ERROR children — they're part of the same error
        return diagnostics

    for child in node.children:
        diagnostics.extend(_walk(child))

    return diagnostics


def _check(source: str) -> list[lsp.Diagnostic]:
    """Parse `source` with tree-sitter and return syntax-error diagnostics."""
    if _PARSER is None:
        return []
    tree = _PARSER.parse(source.encode("utf-8"))
    return _walk(tree.root_node)


# ── LSP server ────────────────────────────────────────────────────────────────

server = LanguageServer(SERVER_NAME, SERVER_VERSION)


def _uri_to_path(uri: str) -> str:
    from urllib.parse import unquote
    return unquote(uri[7:]) if uri.startswith("file://") else uri


def _publish(ls: LanguageServer, uri: str, diagnostics: list[lsp.Diagnostic]) -> None:
    ls.publish_diagnostics(uri, diagnostics)


@server.feature(lsp.TEXT_DOCUMENT_DID_OPEN)
def did_open(ls: LanguageServer, params: lsp.DidOpenTextDocumentParams) -> None:
    doc = params.text_document
    _publish(ls, doc.uri, _check(doc.text))


@server.feature(lsp.TEXT_DOCUMENT_DID_CHANGE)
def did_change(ls: LanguageServer, params: lsp.DidChangeTextDocumentParams) -> None:
    if not params.content_changes:
        return
    # Full sync: last event has the complete current buffer
    last = params.content_changes[-1]
    text = last.text if hasattr(last, "text") else ""
    if text:
        _publish(ls, params.text_document.uri, _check(text))


@server.feature(lsp.TEXT_DOCUMENT_DID_SAVE)
def did_save(ls: LanguageServer, params: lsp.DidSaveTextDocumentParams) -> None:
    path = _uri_to_path(params.text_document.uri)
    try:
        source = Path(path).read_text(encoding="utf-8")
    except OSError:
        return
    _publish(ls, params.text_document.uri, _check(source))


@server.feature(lsp.TEXT_DOCUMENT_DID_CLOSE)
def did_close(ls: LanguageServer, params: lsp.DidCloseTextDocumentParams) -> None:
    _publish(ls, params.text_document.uri, [])


# ── Entry point ───────────────────────────────────────────────────────────────

if __name__ == "__main__":
    print(f"Starting {SERVER_NAME} v{SERVER_VERSION}", file=sys.stderr)
    print(
        f"tree-sitter: {'available' if _PARSER else 'NOT available — no diagnostics'}",
        file=sys.stderr,
    )
    server.start_io()
