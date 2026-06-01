# nvim-mvl

Neovim plugin for [MVL (Maximum Verifiable Language)](../../README.md).

Provides:
- Syntax highlighting via tree-sitter
- Code folding (`zc`, `zo`, `zR`)
- Smart indentation
- Filetype detection for `.mvl` files
- Filetype settings (`commentstring`, indent width, fold method)

## Requirements

- Neovim ≥ 0.9
- [nvim-treesitter](https://github.com/nvim-treesitter/nvim-treesitter)
- A C compiler (`gcc` or `clang`) — needed once to compile the parser

## Installation

### lazy.nvim (recommended)

```lua
-- ~/.config/nvim/lua/plugins/mvl.lua

return {
  -- nvim-treesitter must be loaded first
  {
    "nvim-treesitter/nvim-treesitter",
    build = ":TSUpdate",
  },

  -- MVL language support
  -- Point to the nvim-mvl subdirectory of the mvl_language repo
  {
    dir = "/path/to/mvl_language/etc/nvim-mvl",  -- local path
    -- Or, once published as a standalone repo:
    -- "LAB271/nvim-mvl",
    dependencies = { "nvim-treesitter/nvim-treesitter" },
    ft = "mvl",
    config = function()
      -- Parser is registered automatically via plugin/mvl.lua.
      -- Install it once:
      --   :TSInstall mvl
      --
      -- Or add to nvim-treesitter's ensure_installed:
      --   ensure_installed = { "mvl", ... }
    end,
  },
}
```

After restarting Neovim:

```
:TSInstall mvl
```

### packer.nvim

```lua
use {
  "nvim-treesitter/nvim-treesitter",
  run = ":TSUpdate",
}

use {
  dir = "/path/to/mvl_language/etc/nvim-mvl",
  requires = { "nvim-treesitter/nvim-treesitter" },
  after = "nvim-treesitter",
}
```

### Manual (no plugin manager)

```bash
# Clone the mvl_language repo (if you don't have it)
git clone https://github.com/LAB271/mvl_language.git ~/.local/share/mvl_language

# Symlink the plugin into Neovim's runtime path
ln -s ~/.local/share/mvl_language/etc/nvim-mvl \
      ~/.local/share/nvim/site/pack/mvl/start/nvim-mvl
```

Then in Neovim:
```
:TSInstall mvl
```

## Verify Installation

Run the built-in health check:

```
:checkhealth mvl
```

This checks:
1. Neovim ≥ 0.9
2. nvim-treesitter is installed
3. MVL parser is compiled (`:TSInstall mvl` if missing)
4. `.mvl` filetype detection is working
5. `highlights.scm` is on the runtimepath
6. A C compiler is available (needed for `:TSInstall`)
7. Tree-sitter highlight is active on the current buffer

For deeper inspection, open any `.mvl` file and run:
```
:InspectTree                       -- view the parse tree
:TSHighlightCapturesUnderCursor    -- see highlight groups at cursor
```

## What Gets Highlighted

| Construct | Highlight group |
|-----------|----------------|
| `total`, `partial` | `@keyword.modifier` |
| `public`, `tainted`, `secret` (fn modifiers) | `@keyword.modifier` |
| `iso`, `val`, `ref`, `tag` (capabilities) | `@keyword.modifier` |
| `Public`, `Tainted`, `Secret`, `Clean` (type labels) | `@type.qualifier` |
| `sanitize`, `declassify` | `@keyword.special` |
| `Option`, `Result` | `@type.builtin` |
| `IO`, `Console`, `DB`, `Net`, … | `@keyword.effect` |
| `None`, `true`, `false` | `@constant.builtin` |
| `Some`, `Ok`, `Err` | `@constructor` |
| Function names | `@function` |
| Function calls | `@function.call` |
| Type names | `@type` |
| Type definitions | `@type.definition` |
| Module names | `@module` |
| String literals | `@string` |
| Numbers | `@number` |
| Comments `// …` | `@comment` |

## Colorscheme Notes

The highlight groups follow the standard nvim-treesitter naming convention.
Any colorscheme that supports tree-sitter will render MVL correctly.

`@keyword.modifier` and `@keyword.effect` are custom groups — if your
colorscheme doesn't define them, they fall back to `@keyword`.

To customize:

```lua
-- In your colorscheme or init.lua
vim.api.nvim_set_hl(0, "@keyword.modifier", { fg = "#c792ea", bold = true })
vim.api.nvim_set_hl(0, "@keyword.effect",   { fg = "#82aaff", italic = true })
vim.api.nvim_set_hl(0, "@type.qualifier",   { fg = "#ffcb6b", bold = true })
```

## Known Limitations

- `Public[Int where self ] 0>` — the `>` in a refinement predicate inside
  a generic type bracket causes a parse ambiguity in the context-free grammar.
  Syntax highlighting still works for surrounding code; only the specific
  expression inside the brackets may be highlighted incorrectly.
  Workaround: `type PositiveInt = Int where self > 0` then `Public[PositiveInt]`.

## File Structure

```
etc/nvim-mvl/
├── lua/mvl/init.lua          parser registration with nvim-treesitter
├── plugin/mvl.lua            auto-setup on load
├── after/ftplugin/mvl.lua    filetype settings (commentstring, indent, folds)
└── queries/mvl/
    ├── highlights.scm        syntax highlighting
    ├── folds.scm             code folding
    └── indents.scm           smart indentation
```
