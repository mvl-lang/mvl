# vscode-mvl

VS Code extension for [MVL (Maximum Verifiable Language)](../../README.md).

Provides syntax highlighting, bracket matching, auto-indentation, and
comment toggling via a TextMate grammar (works in all VS Code versions).

## Installation

### From source (local)

```bash
# Install vsce if you don't have it
npm install -g @vscode/vsce

# Package the extension
cd mvl_language/etc/vscode-mvl
vsce package

# Install the generated .vsix
code --install-extension vscode-mvl-0.0.1.vsix
```

Or install without packaging:

1. Copy the `vscode-mvl/` folder to `~/.vscode/extensions/lab271.vscode-mvl-0.0.1/`
2. Restart VS Code

### From the marketplace (future)

> Once published: search "MVL" in the VS Code Extensions view or visit
> the marketplace page.

## Features

| Feature | Notes |
|---------|-------|
| Syntax highlighting | TextMate grammar — works in all VS Code versions |
| Bracket matching | `{}`, `()`, `[]`, strings, chars |
| Auto-close pairs | `{` → `{}`, `"` → `""`, etc. |
| Comment toggle | `Cmd+/` adds/removes `//` |
| Code folding | Based on `{` / `}` markers |
| 4-space indent | Default indent for `.mvl` files |

## Highlighted Constructs

| Construct | VS Code scope | Color (One Dark) |
|-----------|--------------|-----------------|
| `total`, `partial` | `storage.modifier.totality` | Purple |
| `public`, `tainted`, `secret` | `storage.modifier.security` | Purple |
| `iso`, `val`, `ref`, `tag` | `storage.modifier.capability` | Purple |
| `Public[T]`, `Secret[T]`, … | `storage.type.security-label` | Yellow |
| `sanitize`, `declassify` | `keyword.other.special-form` | Red/Pink |
| `IO`, `Console`, `DB`, … | `keyword.other.effect` | Blue |
| `fn`, `type`, `module`, … | `keyword.other.declaration` | Blue |
| `if`, `match`, `let`, … | `keyword.control` | Purple |
| `Option`, `Result`, `Int`, … | `support.type.builtin` | Cyan |
| Type names (`Foo`) | `entity.name.type` | Yellow |
| Function names | `entity.name.function` | Green |
| Function calls | `entity.name.function.call` | Blue |
| `true`, `false`, `None` | `constant.language` | Orange |
| `Some`, `Ok`, `Err` | `entity.name.tag.constructor` | Orange |
| `->`, `=>` | `keyword.operator.arrow` | White |
| `?` | `keyword.operator.propagate` | White |
| Numbers | `constant.numeric` | Orange |
| Strings | `string.quoted.double` | Green |
| Comments | `comment.line` | Grey |

## File Structure

```
etc/vscode-mvl/
├── package.json                 extension manifest
├── language-configuration.json  comments, brackets, auto-pairs, folding
├── README.md
└── syntaxes/
    └── mvl.tmLanguage.json      TextMate grammar (regex-based)
```

## Limitations

- TextMate grammars are regex-based and do not have structural awareness.
  Highlighting is best-effort for complex constructs like nested generic
  types with refinements (`Public[Int where self ] 0>`).

- VS Code 1.93+ has **experimental** tree-sitter support behind a setting.
  A future version of this extension may add tree-sitter queries
  (`queries/mvl/highlights.scm`) for more accurate highlighting.
