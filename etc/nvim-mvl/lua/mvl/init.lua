-- nvim-mvl: Neovim support for MVL (Minimum Verification Language)
-- Registers the tree-sitter parser and filetype with nvim-treesitter.

local M = {}

---Register the MVL parser with nvim-treesitter.
---Called automatically by plugin/mvl.lua on startup.
function M.setup()
  -- Guard: nvim-treesitter must be installed
  local ok, parsers = pcall(require, "nvim-treesitter.parsers")
  if not ok then
    vim.notify(
      "[nvim-mvl] nvim-treesitter is required. Install it first.",
      vim.log.levels.WARN
    )
    return
  end

  local parser_configs = parsers.get_parser_configs()

  -- Only register if not already present (idempotent)
  if parser_configs.mvl then
    return
  end

  parser_configs.mvl = {
    install_info = {
      -- Points to the tree-sitter grammar inside the mvl_language repo.
      -- Change this to the GitHub URL once the repo is public:
      --   url = "https://github.com/LAB271/mvl_language",
      --   files = { "etc/tree-sitter-mvl/src/parser.c" },
      --
      -- For local development, use the absolute path:
      url = vim.fn.fnamemodify(
        vim.api.nvim_get_runtime_file("lua/mvl/init.lua", false)[1],
        ":h:h:h:h"  -- go up: lua/mvl → lua → nvim-mvl → etc → repo root
      ) .. "/etc/tree-sitter-mvl",
      files = { "src/parser.c" },
      generate_requires_npm = false,
      requires_generate_from_grammar = false,
    },
    filetype = "mvl",
    maintainers = { "@LAB271" },
  }

  -- Associate .mvl files with the parser
  vim.filetype.add({
    extension = { mvl = "mvl" },
  })
end

return M
