-- SPDX-License-Identifier: Apache-2.0
-- Copyright 2026 Schuberg Philis
--
-- MVL LSP client for Neovim (Phase 1: diagnostics).
--
-- Launches tools/lsp_server.py from the workspace root.
-- Requires pygls: pip install pygls
--
-- Usage (add to your init.lua after loading nvim-mvl):
--
--   require('mvl').setup({ lsp = true })
--
-- Or with a custom binary path:
--
--   require('mvl').setup({
--     lsp = {
--       python  = '/usr/bin/python3',       -- Python interpreter
--       script  = '/path/to/lsp_server.py', -- Explicit server script
--     }
--   })

local M = {}

--- Resolve the path to lsp_server.py.
-- Search order:
--   1. opts.script (explicit override)
--   2. MVL_LSP_SERVER environment variable
--   3. tools/lsp_server.py relative to the first workspace root
--   4. tools/lsp_server.py relative to this plugin's directory
---@param opts table
---@return string
local function resolve_script(opts)
  if opts.script and opts.script ~= "" then
    return opts.script
  end

  local env_script = os.getenv("MVL_LSP_SERVER")
  if env_script and env_script ~= "" then
    return env_script
  end

  -- cwd-relative (typical for single-repo dev)
  local cwd_candidate = vim.fn.getcwd() .. "/tools/lsp_server.py"
  if vim.fn.filereadable(cwd_candidate) == 1 then
    return cwd_candidate
  end

  -- Relative to plugin directory (installed from the repo)
  local runtime = vim.api.nvim_get_runtime_file("lua/mvl/lsp.lua", false)
  if runtime[1] then
    return vim.fn.fnamemodify(runtime[1], ":h:h:h:h") .. "/tools/lsp_server.py"
  end

  return "tools/lsp_server.py"
end

--- Start the MVL language server for the current buffer.
-- Called automatically via FileType autocmd when lsp is enabled in setup().
---@param opts table  { python?: string, script?: string }
function M.start(opts)
  opts = opts or {}
  local python = opts.python or "python3"
  local script = resolve_script(opts)

  vim.lsp.start({
    name = "mvl-lsp",
    cmd = { python, script },
    root_dir = vim.fs.dirname(
      vim.fs.find({ "Cargo.toml", ".git" }, { upward = true })[1]
    ) or vim.fn.getcwd(),
    filetypes = { "mvl" },
    capabilities = vim.lsp.protocol.make_client_capabilities(),
  })
end

return M
