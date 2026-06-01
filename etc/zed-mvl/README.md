# zed-mvl

Zed extension for [MVL (Maximum Verifiable Language)](../../README.md).

Provides syntax highlighting, bracket matching, and smart indentation via
Zed's native tree-sitter integration.

## Installation

### Development (local)

Zed can load extensions directly from a local directory:

1. Open Zed
2. Open the command palette: `Cmd+Shift+P`
3. Run: `zed: install dev extension`
4. Navigate to and select `mvl_language/etc/zed-mvl/`

The extension loads immediately — open any `.mvl` file to verify.

### Production (extension registry)

> **Note:** Publication to the Zed extension registry requires the
> tree-sitter grammar to be available as a standalone public repository
> (`https://github.com/LAB271/tree-sitter-mvl`). Until then, use the
> local dev install above.

Once published, install via:

1. `Cmd+Shift+P` → `zed: extensions`
2. Search for "MVL"
3. Click Install

## Grammar Note

Zed fetches and compiles the tree-sitter grammar from a Git repository.
The grammar source is currently embedded in the `mvl_language` monorepo at
`etc/tree-sitter-mvl/`. For the extension registry, `grammar.js` must be
at the root of a dedicated repo.

`extension.toml` currently points to the monorepo commit where the grammar
lives — update the `commit` field when the grammar is promoted to a
standalone `tree-sitter-mvl` repo.

## Features

- **Syntax highlighting** — keywords, types, security labels (`Public`/`Tainted`/`Secret`/`Clean`),
  capabilities (`iso`/`val`/`ref`/`tag`), totality modifiers (`total`/`partial`),
  effects (`! Console`, `! DB`), special forms (`sanitize`, `declassify`), operators
- **Bracket matching** — `{}`, `()`, `[]`, strings, char literals
- **Smart indentation** — increases after `{`, decreases on `}`
- **File type detection** — `.mvl` files automatically use MVL mode

## File Structure

```
etc/zed-mvl/
├── extension.toml              extension manifest + grammar Git ref
└── languages/
    └── mvl/
        ├── config.toml         file patterns, comments, bracket pairs
        ├── highlights.scm      tree-sitter highlight queries
        └── indents.scm         smart indentation queries
```
