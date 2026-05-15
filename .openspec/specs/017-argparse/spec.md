---
domain: stdlib
version: 0.1.0
status: draft
date: 2026-05-15
---

# 017 — Argument Parsing (`std.args`)

The MVL argument-parsing library provides struct-driven CLI argument handling with
uniform error messages, auto-generated usage, and type-safe access to command-line
values.  The design principle is: **the struct IS the argument specification** — no
builder pattern, no DSL, no macros.

## Philosophy

Every MVL CLI program faces the same three problems: parsing, validation, and error
reporting.  `std.args` solves all three in one call.  The struct shape drives the
parser; the field types encode the argument kind; refinement predicates express
validity constraints.  The result is uniformity: every program that uses `parse[T]()`
gets consistent error messages and `-h/--help` for free.

Defaults are expressed via `Option[T]` + `.unwrap_or(default)` on the parsed struct —
no special syntax needed.

CLI input is externally-controlled and arrives as `Tainted[String]`.  The standard
library sanitizes it internally; callers receive plain `T` values after parsing.

**Implementation:** `std/args.mvl`, `runtime/rust/src/stdlib/args.rs`,
`src/mvl/backends/rust/emit_types.rs`

**Issue:** #752

## Syntax Overview

```mvl
use std.args.{parse}

type Args = struct {
    host:    String               // required named flag   --host <value>
    port:    Option[Int]          // optional named flag   --port <value>
    verbose: Bool                 // presence flag         --verbose
    input:   Positional[String]   // required positional   first bare arg
    count:   Option[Positional[Int]]  // optional positional  second bare arg
}

fn main() -> Unit ! Console {
    let args: Args = parse[Args]().unwrap_or_exit();
    let port: Int = args.port.unwrap_or(8080);
    // ...
}
```

## Field Kind Summary

| Field type              | Argument kind             | Absent behaviour   |
|-------------------------|---------------------------|--------------------|
| `T`                     | Required named `--name`   | `Err` with usage   |
| `Option[T]`             | Optional named `--name`   | `None`             |
| `Bool`                  | Presence flag `--name`    | `false`            |
| `Positional[T]`         | Required bare token       | `Err` with usage   |
| `Option[Positional[T]]` | Optional bare token       | `None`             |

Positional fields are consumed left-to-right in field-declaration order, before
any named flags are processed.

## Requirements

### Requirement 1: Struct-derived parsing [MUST]

`parse[T]()` MUST derive a complete CLI parser from the struct `T` at transpile time.
The compiler MUST generate an `impl ParseFromArgs for T` for every struct that appears
as the type parameter of `parse[T]()`.  No runtime reflection, no macros, no builder.

**Implementation:** `src/mvl/backends/rust/emit_types.rs::emit_parse_from_args_impl`,
`runtime/rust/src/stdlib/args.rs::ParseFromArgs`

**Tests:** `tests/transpiler.rs::argparse_required_flag_generates_parse_impl`,
`tests/transpiler.rs::argparse_optional_flag_generates_option_parse`

#### Scenario: Struct drives the parser

- GIVEN a struct `type Cfg = struct { name: String }`
- WHEN `parse[Cfg]()` is called
- THEN the compiler MUST generate a `ParseFromArgs` impl that parses `--name <value>`
- AND return `Err(usage)` if `--name` is absent

#### Scenario: Unknown struct field type rejected

- GIVEN a struct field with type `Channel[Int]` (not a parseable scalar)
- WHEN `parse[Cfg]()` is compiled
- THEN the compiler MUST NOT generate a parser for that field (field is silently skipped
  or a compile error is emitted — behaviour TBD in Phase 6 refinement)

---

### Requirement 2: Positional arguments [MUST]

Fields typed `Positional[T]` MUST be parsed from leading non-flag argv tokens in
field-declaration order.  `Positional[T]` is a compile-time annotation only; at
runtime the value is plain `T`.

**Implementation:** `src/mvl/backends/rust/emit_types.rs::emit_positional_field_parse`,
`src/mvl/backends/rust/emit_types.rs::emit_required_positional`,
`src/mvl/backends/rust/emit_types.rs::emit_optional_positional`

**Tests:** `tests/transpiler.rs::argparse_positional_field_generates_positional_parse`,
`tests/transpiler.rs::argparse_optional_positional_generates_option_parse`

#### Scenario: Required positional present

- GIVEN `type Args = struct { file: Positional[String] }`
- WHEN the program is invoked as `prog myfile.txt`
- THEN `args.file` MUST equal `"myfile.txt"`

#### Scenario: Required positional absent

- GIVEN `type Args = struct { file: Positional[String] }`
- WHEN the program is invoked as `prog` (no arguments)
- THEN `parse[Args]()` MUST return `Err(usage_message)`

#### Scenario: Optional positional absent

- GIVEN `type Args = struct { count: Option[Positional[Int]] }`
- WHEN the program is invoked with no arguments
- THEN `args.count` MUST be `None`

#### Scenario: Positional order

- GIVEN `type Args = struct { src: Positional[String], dst: Positional[String] }`
- WHEN the program is invoked as `prog a.txt b.txt`
- THEN `args.src` MUST be `"a.txt"` and `args.dst` MUST be `"b.txt"`

---

### Requirement 3: Named flag arguments [MUST]

Fields typed `T` (not `Bool`, not `Positional`) MUST be parsed from `--<name> <value>`
or `--<name>=<value>` flag pairs.  Required fields (non-`Option`) MUST produce `Err`
if absent.  Optional fields (`Option[T]`) MUST produce `None` if absent.

**Implementation:** `src/mvl/backends/rust/emit_types.rs::emit_parse_from_args_impl`

**Tests:** `tests/transpiler.rs::argparse_required_flag_generates_parse_impl`,
`tests/transpiler.rs::argparse_optional_flag_generates_option_parse`,
`tests/transpiler.rs::argparse_error_on_missing_required`

#### Scenario: Required flag present

- GIVEN `type Args = struct { host: String }`
- WHEN invoked as `prog --host localhost`
- THEN `args.host` MUST equal `"localhost"`

#### Scenario: Required flag absent

- GIVEN `type Args = struct { host: String }`
- WHEN invoked as `prog` (no arguments)
- THEN `parse[Args]()` MUST return `Err("missing required flag --host\n\nusage: ...")`

#### Scenario: Optional flag absent

- GIVEN `type Args = struct { port: Option[Int] }`
- WHEN invoked without `--port`
- THEN `args.port` MUST be `None`

---

### Requirement 4: Boolean presence flags [MUST]

Fields typed `Bool` MUST be parsed as presence flags: `true` if the flag appears in
argv, `false` if absent.  No value token follows the flag.

**Implementation:** `src/mvl/backends/rust/emit_types.rs::emit_parse_from_args_impl`

**Tests:** `tests/transpiler.rs::argparse_bool_flag_generates_presence_check`

#### Scenario: Boolean flag present

- GIVEN `type Args = struct { verbose: Bool }`
- WHEN invoked as `prog --verbose`
- THEN `args.verbose` MUST be `true`

#### Scenario: Boolean flag absent

- GIVEN `type Args = struct { verbose: Bool }`
- WHEN invoked without `--verbose`
- THEN `args.verbose` MUST be `false`

---

### Requirement 5: Auto-generated help [MUST]

`parse[T]()` MUST respond to `-h` or `--help` by printing a usage string to stdout
and exiting with code 0.  The usage string MUST be derived from the struct field names
and types.  No user code is required to enable this behaviour.

**Implementation:** `src/mvl/backends/rust/emit_types.rs::emit_usage_string`,
`src/mvl/backends/rust/emit_types.rs::emit_parse_from_args_impl`

**Tests:** `tests/transpiler.rs::argparse_help_flag_emits_exit_0`

#### Scenario: -h exits cleanly

- GIVEN any struct `T` used with `parse[T]()`
- WHEN the program is invoked as `prog -h` or `prog --help`
- THEN the program MUST print the usage string and exit with code 0
- AND MUST NOT return an `Err` value

#### Scenario: Usage lists all fields

- GIVEN `type Args = struct { file: Positional[String], verbose: Bool, port: Option[Int] }`
- WHEN `-h` is passed
- THEN the usage string MUST mention `<file>`, `[--verbose]`, and `[--port <Int>]`

---

### Requirement 6: Uniform error handling [MUST]

`parse[T]()` MUST return `Result[T, String]`.  On failure the `Err` string MUST
include the reason and the auto-generated usage.  `unwrap_or_exit()` MUST print the
error to stderr and exit with code 1 when called on an `Err` value.

**Implementation:** `runtime/rust/src/stdlib/args.rs::unwrap_or_exit`,
`std/args.mvl::parse`

**Tests:** `tests/transpiler.rs::argparse_error_on_missing_required`

#### Scenario: unwrap_or_exit on Err

- GIVEN `parse[Args]()` returns `Err("missing required flag --host\n...")`
- WHEN `.unwrap_or_exit()` is called
- THEN the program MUST print `"error: missing required flag --host\n..."` to stderr
- AND exit with code 1

#### Scenario: unwrap_or_exit on Ok

- GIVEN `parse[Args]()` returns `Ok(args)`
- WHEN `.unwrap_or_exit()` is called
- THEN `args` MUST be returned with no side effects

---

### Requirement 7: Positional[T] type transparency [MUST]

`Positional[T]` MUST be transparent to the type system: a value of type `Positional[T]`
MUST satisfy any context expecting `T`, and a value of type `T` MUST satisfy any context
expecting `Positional[T]`.  The Rust emitter MUST emit `T` (not `Positional<T>`) for
struct fields typed `Positional[T]`.

**Implementation:** `src/mvl/checker/types.rs::types_compatible` (transparency rules),
`src/mvl/backends/rust/emit_types.rs::emit_type_expr` (transparent unwrapping),
`src/mvl/backends/rust/emitter.rs` (`"Positional"` in builtins set)

**Tests:** `tests/transpiler.rs::argparse_positional_field_type_is_transparent`

#### Scenario: Positional[Int] satisfies Int slot

- GIVEN `type Args = struct { n: Option[Positional[Int]] }`
- WHEN `args.n.unwrap_or(5)` is written (default `5` is `Int`, field is `Option[Positional[Int]]`)
- THEN the type checker MUST accept the expression
- AND the emitted Rust MUST type-check without `Positional<i64>` in the struct

#### Scenario: Positional not emitted as extern stub

- GIVEN any MVL program using `Positional[T]`
- WHEN the Rust backend emits the program
- THEN `struct Positional` MUST NOT appear in the emitted output
- AND the struct field MUST be emitted as the inner type `T`

---

### Requirement 8: Refinement validation at parse time [SHOULD]

When a field type carries a refinement predicate (e.g. `Int where self > 0`), the
generated parser SHOULD validate the predicate after parsing the raw string and return
`Err` if the predicate is not satisfied.

**Implementation:** `src/mvl/backends/rust/emit_types.rs::emit_parse_from_args_impl`
(refinement check emission — Phase 6)

**Tests:** pending Phase 6 SMT integration

#### Scenario: Refined positional rejected

- GIVEN `type Args = struct { rounds: Positional[Int where self > 0] }`
- WHEN invoked as `prog 0` or `prog -5`
- THEN `parse[Args]()` MUST return `Err("rounds: expected value > 0")`

#### Scenario: Refined positional accepted

- GIVEN `type Args = struct { rounds: Positional[Int where self > 0] }`
- WHEN invoked as `prog 5`
- THEN `parse[Args]()` MUST return `Ok(Args { rounds: 5 })`
