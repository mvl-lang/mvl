-- nvim-mvl: Neovim support for MVL (Minimum Verification Language)
-- Registers the tree-sitter parser and filetype with nvim-treesitter.

local M = {}

---Register the MVL parser with nvim-treesitter and optionally start the LSP.
---Called automatically by plugin/mvl.lua on startup.
---
---Works with:
---  - nvim-treesitter master branch (get_parser_configs API)
---  - nvim-treesitter main branch  (plain-table parsers module)
---
---@param opts? table  Optional config:
---   lsp (boolean|table) — enable LSP diagnostics via tools/lsp_server.py
---     true              — use defaults (python3, auto-discover script)
---     { python, script } — explicit paths
function M.setup(opts)
  -- Resolve grammar path relative to this file
  local runtime_files = vim.api.nvim_get_runtime_file("lua/mvl/init.lua", false)
  local grammar_path = runtime_files[1]
    and vim.fn.fnamemodify(runtime_files[1], ":h:h:h:h") .. "/tree-sitter-mvl"
    or "https://github.com/LAB271/mvl_language"

  local mvl_config = {
    install_info = {
      url = grammar_path,
      files = { "src/parser.c" },
      generate_requires_npm = false,
      requires_generate_from_grammar = false,
    },
    filetype = "mvl",
    maintainers = { "@LAB271" },
  }

  local ok, parsers = pcall(require, "nvim-treesitter.parsers")
  if ok and type(parsers) == "table" then
    if type(parsers.get_parser_configs) == "function" then
      -- Old API (master branch)
      local configs = parsers.get_parser_configs()
      if not configs.mvl then
        configs.mvl = mvl_config
      end
    else
      -- New API (main branch): parsers is a plain table
      if not parsers.mvl then
        parsers.mvl = mvl_config
      end
    end
  end

  -- Associate .mvl files with the parser
  vim.filetype.add({ extension = { mvl = "mvl" } })

  -- Optional LSP: start the language server on every MVL buffer open
  if opts and opts.lsp then
    local lsp_opts = type(opts.lsp) == "table" and opts.lsp or {}
    vim.api.nvim_create_autocmd("FileType", {
      pattern = "mvl",
      callback = function()
        require("mvl.lsp").start(lsp_opts)
      end,
    })
  end
end

return M
