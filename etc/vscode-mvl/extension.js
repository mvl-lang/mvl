// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis
//
// MVL VS Code extension — LSP client bootstrap (Phase 1: tree-sitter syntax diagnostics).
//
// Setup:
//   cd etc/vscode-mvl && npm install
//   pip install pygls tree-sitter
//   pip install ./etc/tree-sitter-mvl   # or let the server build from parser.c
//
// The extension launches tools/lsp_server.py as the language server.
// For full type/effect diagnostics see Phase 2 (issue #1003).
// Override the script path via `mvl.lspServerScript` or MVL_LSP_SERVER env var.

"use strict";

const path = require("path");
const { workspace, window } = require("vscode");
const {
  LanguageClient,
  TransportKind,
} = require("vscode-languageclient/node");

/** @type {LanguageClient | undefined} */
let client;

/**
 * Resolve path to lsp_server.py:
 *  1. mvl.lspServerScript setting
 *  2. MVL_LSP_SERVER env var
 *  3. <workspaceFolder>/tools/lsp_server.py   (development default)
 *  4. <extensionDir>/../../tools/lsp_server.py (installed from repo)
 *
 * @param {import("vscode").ExtensionContext} context
 * @returns {string}
 */
function resolveServerScript(context) {
  const config = workspace.getConfiguration("mvl");
  const setting = config.get("lspServerScript", "");
  if (setting) return setting;

  if (process.env.MVL_LSP_SERVER) return process.env.MVL_LSP_SERVER;

  const roots = workspace.workspaceFolders;
  if (roots && roots.length > 0) {
    const candidate = path.join(roots[0].uri.fsPath, "tools", "lsp_server.py");
    return candidate;
  }

  // Fallback: relative to this extension file (works when installed from repo)
  return path.join(context.extensionPath, "..", "..", "tools", "lsp_server.py");
}

/**
 * @param {import("vscode").ExtensionContext} context
 */
async function activate(context) {
  const config = workspace.getConfiguration("mvl");
  if (!config.get("lspEnabled", true)) return;

  const python = config.get("pythonPath", "python3");
  const serverScript = resolveServerScript(context);

  const serverOptions = {
    command: python,
    args: [serverScript],
    transport: TransportKind.stdio,
  };

  const clientOptions = {
    documentSelector: [{ scheme: "file", language: "mvl" }],
    synchronize: {
      fileEvents: workspace.createFileSystemWatcher("**/*.mvl"),
    },
  };

  client = new LanguageClient(
    "mvl-lsp",
    "MVL Language Server",
    serverOptions,
    clientOptions
  );

  await client.start();
  window.setStatusBarMessage("MVL LSP started", 3000);
}

async function deactivate() {
  if (client) {
    await client.stop();
    client = undefined;
  }
}

module.exports = { activate, deactivate };
