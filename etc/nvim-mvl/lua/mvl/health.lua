-- :checkhealth mvl  — diagnostics for the nvim-mvl plugin

local M = {}

local h = vim.health or require("health")
local ok    = h.ok    or h.report_ok
local warn  = h.warn  or h.report_warn
local error = h.error or h.report_error
local start = h.start or h.report_start

function M.check()
  -- ── 1. Neovim version ────────────────────────────────────────────────
  start("Neovim version")
  if vim.fn.has("nvim-0.9") == 1 then
    ok("Neovim ≥ 0.9 ✓")
  else
    error("Neovim ≥ 0.9 required (nvim-treesitter highlight API)")
  end

  -- ── 2. nvim-treesitter ───────────────────────────────────────────────
  start("nvim-treesitter")
  local ts_ok, ts = pcall(require, "nvim-treesitter")
  if ts_ok then
    ok("nvim-treesitter is installed")
  else
    error("nvim-treesitter not found — install it first", {
      "lazy.nvim:  { 'nvim-treesitter/nvim-treesitter', build = ':TSUpdate' }",
    })
    return  -- nothing else will work without this
  end

  -- ── 3. MVL parser installed ──────────────────────────────────────────
  start("MVL tree-sitter parser")
  local parser_ok = pcall(vim.treesitter.language.inspect, "mvl")
  if parser_ok then
    ok("Parser 'mvl' is compiled and loaded")
  else
    error("Parser 'mvl' is NOT installed", {
      "Run:  :TSInstall mvl",
    })
  end

  -- ── 4. Filetype detection ────────────────────────────────────────────
  start("Filetype detection (.mvl)")
  local ft = vim.filetype.match({ filename = "test.mvl" })
  if ft == "mvl" then
    ok("'.mvl' files detected as filetype 'mvl'")
  else
    warn("'.mvl' not mapped to filetype 'mvl' (got: " .. tostring(ft) .. ")", {
      "Ensure plugin/mvl.lua is sourced (vim.filetype.add is called there)",
    })
  end

  -- ── 5. Highlight queries ─────────────────────────────────────────────
  start("Highlight queries")
  local query_files = vim.api.nvim_get_runtime_file("queries/mvl/highlights.scm", true)
  if #query_files > 0 then
    ok("highlights.scm found: " .. query_files[1])
  else
    error("queries/mvl/highlights.scm not found in runtimepath", {
      "Make sure etc/nvim-mvl is on your runtimepath or loaded by your plugin manager",
    })
  end

  -- ── 6. C compiler (needed for TSInstall) ────────────────────────────
  start("C compiler (for :TSInstall)")
  local cc = vim.fn.exepath("cc") ~= "" and "cc"
    or vim.fn.exepath("gcc") ~= "" and "gcc"
    or vim.fn.exepath("clang") ~= "" and "clang"
    or nil
  if cc then
    ok("C compiler found: " .. cc)
  else
    warn("No C compiler (cc/gcc/clang) found — needed to compile the parser via :TSInstall")
  end

  -- ── 7. Current buffer (if .mvl) ─────────────────────────────────────
  start("Current buffer")
  local buf_ft = vim.bo.filetype
  local buf_name = vim.api.nvim_buf_get_name(0)
  local is_mvl_file = buf_name:match("%.mvl$") ~= nil
  if buf_ft == "mvl" then
    ok("Current buffer filetype = mvl")
    local hl_active = pcall(vim.treesitter.get_parser, 0, "mvl")
    if hl_active then
      ok("Tree-sitter parser active on this buffer")
    else
      warn("Tree-sitter parser not active on this buffer", {
        "Run:  :TSBufEnable highlight",
      })
    end
  elseif is_mvl_file then
    -- A .mvl file is open but filetype wasn't detected — real problem
    error("Buffer is a .mvl file but filetype=" .. buf_ft, {
      "Ensure require('mvl').setup() is called in your init.lua",
    })
  else
    ok("No .mvl file open — buffer checks skipped")
  end
end

return M
