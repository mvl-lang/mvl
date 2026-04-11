-- MVL filetype settings
-- Applied after all other ftplugin files when a .mvl file is opened.

local opt = vim.opt_local

-- Comments: MVL uses // for line comments
opt.commentstring = "// %s"

-- Indentation: 4-space indent (matches corpus examples)
opt.shiftwidth = 4
opt.tabstop = 4
opt.expandtab = true
opt.softtabstop = 4

-- Wrap behaviour: MVL function signatures can be long
opt.textwidth = 100
opt.wrap = false

-- Enable tree-sitter folding if available
if pcall(require, "nvim-treesitter") then
  opt.foldmethod = "expr"
  opt.foldexpr = "nvim_treesitter#foldexpr()"
  opt.foldenable = false  -- open all folds by default
end

-- Basic word boundary characters (include _ for snake_case identifiers)
opt.iskeyword:append("_")
