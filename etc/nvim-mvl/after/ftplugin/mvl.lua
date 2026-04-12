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

-- Enable tree-sitter highlighting and folding
vim.treesitter.start()
opt.foldmethod = "expr"
opt.foldexpr = "v:lua.vim.treesitter.foldexpr()"
opt.foldenable = false  -- open all folds by default

-- Basic word boundary characters (include _ for snake_case identifiers)
opt.iskeyword:append("_")
