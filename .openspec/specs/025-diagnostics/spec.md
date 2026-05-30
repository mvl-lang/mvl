---
domain: toolchain
version: 0.1.0
status: accepted
date: 2026-05-30
---

# 025 — Compiler Diagnostic Format

The MVL compiler emits diagnostics (errors) in a source-context format modelled
after rustc.  Every error produced by `mvl check` or `mvl build` MUST include
the offending source line, a caret underline, and a structured header.

## Requirements

### Requirement 1: Source-Context Error Format [MUST]

Every compiler error SHALL be rendered in the following multi-line format:

```
error[REQ{N}]: {title}
 --> {file}:{line}:{col}
  |
{line} | {source_line}
  |    {carets} {annotation}
```

Where:
- `{N}` — the violated requirement number (1–11)
- `{title}` — short description before the first `": "` in the error message
- `{file}:{line}:{col}` — 1-based file path, line, and column from the span
- `{source_line}` — the full source line at that location
- `{carets}` — `^` characters spanning `Span.col` to `Span.col + Span.len`
- `{annotation}` — detail after the first `": "` in the error message, or the
  full message when no colon separator is present

**Implementation:** `src/cli.rs::render_diagnostic`

#### Scenario: Refinement violation in build

- GIVEN `fn double(x: Int where x > 0)` and a call `double(-2)`
- WHEN `mvl build` or `mvl check` is run
- THEN the error output is:

```
error[REQ10]: refinement predicate violated
 --> main.mvl:6:23
  |
6 |     let result: Int = double(-2);
  |                       ^^^^^^^^^^ argument to `double` violates refinement `self > 0`
```

**Tests:** `examples/fail/main.mvl`

### Requirement 2: Line Number Alignment [MUST]

The `-->` arrow, blank gutter `|` lines, and caret line SHALL be aligned to the
same column as the source line's right-justified line number.

For a file with a single-digit line number (width = 1):
```
 --> file.mvl:5:3
  |
5 | source
  | ^^^ annotation
```

For a two-digit line number (width = 2):
```
  --> file.mvl:12:3
   |
12 | source
   | ^^^ annotation
```

**Implementation:** `src/cli.rs::render_diagnostic` (`line_pad`, `gutter` variables)

### Requirement 3: Error Code Casing [MUST]

Error codes in the header SHALL use uppercase `REQ` prefix: `error[REQ10]`.
The format is `error[REQ{N}]` where `{N}` is the bare requirement number
(no zero-padding).

**Implementation:** `src/cli.rs::render_diagnostic`

### Requirement 4: Message Split Convention [SHOULD]

Error messages that have a colon-separated structure (`title: detail`) SHOULD
use `": "` as the split point.  The `title` portion appears on the header line;
the `detail` portion appears as the inline annotation on the caret line.

Messages without a colon separator use the full message for both header and
annotation.

Double-backtick wrapping in messages MUST be avoided: when the detail string
already contains backtick-quoted identifiers, the outer message format MUST NOT
add additional backtick wrapping.

**Implementation:** `src/cli.rs::render_diagnostic`, `src/mvl/checker/errors.rs`

### Requirement 5: Caret Span Accuracy [SHOULD]

The caret underline SHOULD span the token or expression identified by the
error's `Span`.  `Span.len` gives the byte length of the relevant token;
a minimum of 1 caret is always shown.

When `Span` covers a call expression, all carets span the full call site.
Pinpointing individual sub-expressions (e.g. only the argument) is out of scope
for this specification.

**Implementation:** `src/cli.rs::render_diagnostic` (`caret_len` variable)

### Requirement 6: JSON Format Unchanged [MUST]

When `mvl check --format=json` is used, errors SHALL continue to be emitted as
structured JSON objects and SHALL NOT use the source-context format.  The JSON
format is a separate output mode.

**Implementation:** `src/cli/check.rs` (json branch, lines ~269–291)

## Known Limitations

- **L1**: The caret underlines the full call expression, not the specific
  argument that violated the refinement.  Narrowing to sub-expressions requires
  storing argument spans separately in `CheckError`.
- **L2**: Errors from stdlib files in `--stdlib=proven` mode still use the
  single-line format (`std/{name}:line:col: error[req{N}]: message`) because
  stdlib source text is not passed through the check path.
- **L3**: `render_diagnostic` is not yet used by `run_stdin` in `check.rs`.
