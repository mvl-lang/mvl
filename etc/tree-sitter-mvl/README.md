# tree-sitter-mvl

Tree-sitter grammar for [MVL (Maximum Verifiable Language)](../../README.md) — a minimal,
compiler-verified language designed for safety-critical and security-sensitive code generation.

Provides syntax highlighting for **Zed** and **Neovim** (and any editor backed by tree-sitter).

## Features

- Full MVL grammar (`grammar.js`) derived from the formal EBNF (`docs/grammar.ebnf`)
- Highlight queries (`queries/highlights.scm`) covering:
  - Keywords: `fn`, `type`, `module`, `let`, `match`, `where`, …
  - Totality modifiers: `total` / `partial`
  - Security modifiers: `public` / `tainted` / `secret`
  - Capability annotations: `iso` / `val` / `ref` / `tag`
  - Security labels: `Public[T]` / `Tainted[T]` / `Secret[T]` / `Clean[T]`
  - Built-in generic types: `Option[T]` / `Result[T, E]`
  - Effects: `! IO`, `! Console`, `! DB`, …
  - Special forms: `sanitize()` / `declassify()` / `?` propagation
- Code folding queries (`queries/folds.scm`)
- Tree-sitter test corpus (`corpus/`)

## Prerequisites

```bash
npm install -g tree-sitter-cli   # or: cargo install tree-sitter-cli
```

## Build

```bash
cd etc/tree-sitter-mvl
npm install
npm run build          # runs: tree-sitter generate && node-gyp build
npm test               # runs corpus tests
```

Or via the project Makefile:

```bash
make tree-sitter-build
make tree-sitter-test
```

## Installation

### Zed

1. Build the grammar (see above) — `src/parser.c` must exist.

2. Create a Zed extension directory:

```
~/.config/zed/extensions/tree-sitter-mvl/
├── extension.toml
├── grammars/
│   └── mvl.toml
└── languages/
    └── mvl/
        ├── config.toml
        └── highlights.scm  → symlink or copy from queries/highlights.scm
```

3. `extension.toml`:

```toml
id = "mvl"
name = "MVL"
version = "0.1.0"
description = "Maximum Verifiable Language support for Zed"
authors = ["MVL Team"]
repository = "https://github.com/your-org/mvl_language"

[grammars.mvl]
repository = "https://github.com/your-org/tree-sitter-mvl"
commit = "<latest-commit-sha>"

[languages.mvl]
name = "MVL"
grammar = "mvl"
path_suffixes = ["mvl"]
line_comments = ["//"]
```

4. `languages/mvl/config.toml`:

```toml
name = "MVL"
line_comments = ["//"]
block_comment = []
path_suffixes = ["mvl"]
```

5. Restart Zed — the extension will be picked up automatically.

### Neovim (nvim-treesitter)

1. Add the parser to your nvim-treesitter configuration:

```lua
-- In your Neovim config (init.lua or plugins/treesitter.lua)
local parser_config = require("nvim-treesitter.parsers").get_parser_configs()

parser_config.mvl = {
  install_info = {
    -- Point to local grammar or GitHub URL after publishing
    url = "/path/to/mvl_language/etc/tree-sitter-mvl",
    files = { "src/parser.c" },
    branch = "main",
  },
  filetype = "mvl",
}

-- Associate .mvl files with the parser
vim.filetype.add({
  extension = { mvl = "mvl" },
})
```

2. Install the parser:

```
:TSInstall mvl
```

3. Copy or symlink highlight queries:

```bash
mkdir -p ~/.config/nvim/queries/mvl
cp etc/tree-sitter-mvl/queries/highlights.scm ~/.config/nvim/queries/mvl/
cp etc/tree-sitter-mvl/queries/folds.scm ~/.config/nvim/queries/mvl/
```

4. Open any `.mvl` file — syntax highlighting will be active.

### VS Code (TextMate fallback)

VS Code does not natively support tree-sitter (as of 2025). A TextMate grammar (regex-based)
can be added as a separate `etc/vscode-mvl/` extension if needed. The tree-sitter grammar
is the canonical definition — any TextMate grammar should be derived from it.

## Grammar Overview

The MVL grammar is **LL(1)** — parseable by recursive descent with one token of lookahead.
It maps cleanly to tree-sitter's context-free grammar DSL with no ambiguities.

| Category | Examples |
|----------|---------|
| Declarations | `module`, `type`, `fn`, `const` |
| Totality | `total`, `partial` |
| Security (function) | `public`, `tainted`, `secret` |
| Capabilities | `iso`, `val`, `ref`, `tag` |
| Security labels | `Public[T]`, `Tainted[T]`, `Secret[T]`, `Clean[T]` |
| Built-in types | `Option[T]`, `Result[T, E]` |
| Effects | `IO`, `Console`, `FileRead`, `FileWrite`, `Net`, `DB` |
| Special forms | `sanitize()`, `declassify()`, `?` propagation |

See [`docs/grammar.ebnf`](../../docs/grammar.ebnf) for the complete formal grammar.

## Known Limitations

### `>` in refinements inside generics

The expression `Public[Int where self ] 0>` causes a parse error because tree-sitter's
context-free parser cannot distinguish the `>` in `self > 0` from the closing `>` of
the generic bracket. This is the classic "angle bracket" ambiguity.

**Workaround:** Use `>=` when possible (`self >= 1` instead of `self > 0`), or extract
the type alias:

```mvl
// ❌ Ambiguous for the tree-sitter grammar:
num_parties: Public[Int where self ] 0>

// ✅ Works: use alias
type PositiveInt = Int where self > 0
num_parties: Public[PositiveInt]

// ✅ Works: use >= which is a two-character unambiguous token
num_parties: Public[Int where self ]= 1>
```

The MVL compiler itself (hand-written recursive descent) handles this correctly.
This limitation only applies to the tree-sitter grammar used for syntax highlighting.

A proper fix would require an external scanner (tree-sitter external API) to track
`<>` nesting depth — planned for a future iteration.

## Development

### Adding test cases

Edit `corpus/basics.txt` or `corpus/full_programs.txt` following the tree-sitter
test format:

```
===============================================================================
Test name
===============================================================================
code here
-------------------------------------------------------------------------------
(expected_parse_tree)
```

Run `npm test` (or `tree-sitter test`) to validate.

### Updating the grammar

1. Edit `grammar.js`
2. Run `tree-sitter generate` to regenerate `src/parser.c`
3. Run `npm test` to verify corpus tests pass
4. Update highlight queries if new node types were added
