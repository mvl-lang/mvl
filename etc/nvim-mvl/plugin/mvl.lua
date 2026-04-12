-- Auto-setup: runs when the plugin is loaded by lazy.nvim / packer.
-- Registers the MVL parser with nvim-treesitter so `:TSInstall mvl` works.

if vim.g.loaded_nvim_mvl then
  return
end
vim.g.loaded_nvim_mvl = true

require("mvl").setup()

vim.api.nvim_create_autocmd("FileType", {
  pattern = "mvl",
  callback = function()
    vim.treesitter.start()
  end,
})
