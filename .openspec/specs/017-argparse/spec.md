---
domain: stdlib
version: 0.2.0
status: draft
date: 2026-05-16
---

# 017 — Argument Parsing (`std.args`)

The MVL argument-parsing library provides explicit schema-driven CLI argument
handling with uniform error messages and auto-generated usage text.

## Philosophy

The schema IS the argument specification — passed as a plain `List[FieldSpec]`
value.  No struct introspection, no codegen, no macros.  The parser is pure MVL
built on `std.env`.

Argument failures are always fatal for a CLI program.  `parse_args` therefore
never returns a `Result` — it exits the process on failure and returns the map
on success.  This removes the `unwrap_or_exit` ceremony from every call site.

CLI input is externally-controlled and arrives as `Tainted[String]`.  The
parser sanitizes it internally; callers receive plain `ArgValue` variants.

**Implementation:** `std/args.mvl`

**Issue:** #752, #815

## Types

```mvl
pub type ArgType = enum { Str, Int, Float }

pub type FieldSpec = enum {
    Required         { name: String, ty: ArgType }
    Optional         { name: String, ty: ArgType }
    Flag             { name: String }
    Positional       { name: String, ty: ArgType }
    OptPositional    { name: String, ty: ArgType }
}

pub type ArgValue = enum {
    Str(String),
    Int(Int),
    Float(Float),
    Bool(Bool),
}
```

`Bool` is not an `ArgType` because boolean fields are always presence flags —
there is no `--verbose true` syntax.  `Flag` in the schema implies `Bool` in
the output map.

## Schema Constructors

```mvl
pub fn required(name: String, ty: ArgType)     -> FieldSpec
pub fn optional(name: String, ty: ArgType)     -> FieldSpec
pub fn flag(name: String)                      -> FieldSpec
pub fn positional(name: String, ty: ArgType)   -> FieldSpec
pub fn opt_positional(name: String, ty: ArgType) -> FieldSpec
```

## Field Kind Summary

| Constructor        | Argument kind            | Absent behaviour        |
|--------------------|--------------------------|-------------------------|
| `required`         | Named `--name <value>`   | print error+usage, exit 1 |
| `optional`         | Named `--name <value>`   | key absent from map     |
| `flag`             | Presence `--name`        | `Bool(false)` in map    |
| `positional`       | Leading bare token       | print error+usage, exit 1 |
| `opt_positional`   | Leading bare token       | key absent from map     |

Positional fields are consumed left-to-right in schema-declaration order.

## Syntax Overview

```mvl
use std.args.{parse_args, required, optional, flag, positional, ArgValue, ArgType}

fn main() -> Unit ! Console {
    let args = parse_args([
        required("host",    ArgType::Str),
        optional("port",    ArgType::Int),
        flag("verbose"),
        positional("input", ArgType::Str),
    ])
    let host = match args.get("host") { Some(ArgValue::Str(s)) => s, _ => "" }
    let port = match args.get("port") { Some(ArgValue::Int(n)) => n, _ => 8080 }
    let verbose = match args.get("verbose") { Some(ArgValue::Bool(b)) => b, _ => false }
}
```

## Requirements

### Requirement 1: Schema-driven parsing [MUST]

`parse_args` MUST accept a `List[FieldSpec]` and parse `std.env.args()` according
to that schema.  No struct introspection, no code generation, no runtime reflection.
The implementation MUST be pure MVL using `std.env` and `std.io` as its only
builtins.

**Implementation:** `std/args.mvl::parse_args`

#### Scenario: Named flag parsed

- GIVEN schema `[required("host", ArgType::Str)]`
- WHEN invoked as `prog --host localhost`
- THEN `args.get("host")` MUST equal `Some(ArgValue::Str("localhost"))`

#### Scenario: `--name=value` form

- GIVEN schema `[required("host", ArgType::Str)]`
- WHEN invoked as `prog --host=localhost`
- THEN `args.get("host")` MUST equal `Some(ArgValue::Str("localhost"))`

---

### Requirement 2: Positional arguments [MUST]

`positional` and `opt_positional` fields MUST be consumed from leading non-flag
argv tokens in schema-declaration order.

**Implementation:** `std/args.mvl::parse_args`

#### Scenario: Required positional present

- GIVEN schema `[positional("file", ArgType::Str)]`
- WHEN invoked as `prog myfile.txt`
- THEN `args.get("file")` MUST equal `Some(ArgValue::Str("myfile.txt"))`

#### Scenario: Required positional absent

- GIVEN schema `[positional("file", ArgType::Str)]`
- WHEN invoked as `prog` (no arguments)
- THEN `parse_args` MUST print an error message to stderr and exit with code 1

#### Scenario: Optional positional absent

- GIVEN schema `[opt_positional("count", ArgType::Int)]`
- WHEN invoked with no arguments
- THEN `args.get("count")` MUST be `None`

#### Scenario: Positional order

- GIVEN schema `[positional("src", ArgType::Str), positional("dst", ArgType::Str)]`
- WHEN invoked as `prog a.txt b.txt`
- THEN `args.get("src")` MUST be `Some(ArgValue::Str("a.txt"))`
- AND `args.get("dst")` MUST be `Some(ArgValue::Str("b.txt"))`

---

### Requirement 3: Boolean presence flags [MUST]

`flag` fields MUST be `ArgValue::Bool(true)` when the flag appears in argv and
`ArgValue::Bool(false)` when absent.  No value token follows the flag.

**Implementation:** `std/args.mvl::parse_args`

#### Scenario: Flag present

- GIVEN schema `[flag("verbose")]`
- WHEN invoked as `prog --verbose`
- THEN `args.get("verbose")` MUST equal `Some(ArgValue::Bool(true))`

#### Scenario: Flag absent

- GIVEN schema `[flag("verbose")]`
- WHEN invoked without `--verbose`
- THEN `args.get("verbose")` MUST equal `Some(ArgValue::Bool(false))`

---

### Requirement 4: Exit on failure [MUST]

`parse_args` MUST print a human-readable error message followed by the usage
string to stderr and exit with code 1 when any required field is absent or any
value cannot be coerced to the declared `ArgType`.  It MUST NOT return to the
caller on failure.

**Implementation:** `std/args.mvl::parse_args`

#### Scenario: Missing required flag

- GIVEN schema `[required("host", ArgType::Str)]`
- WHEN invoked as `prog` (no arguments)
- THEN the program MUST print `"error: missing required flag --host"` to stderr
- AND print the usage string
- AND exit with code 1

#### Scenario: Type coercion failure

- GIVEN schema `[required("port", ArgType::Int)]`
- WHEN invoked as `prog --port abc`
- THEN the program MUST print an error indicating `--port` expects an integer
- AND exit with code 1

---

### Requirement 5: Auto-generated help [MUST]

`parse_args` MUST respond to `-h` or `--help` by printing a usage string to
stdout and exiting with code 0.  The usage string MUST be derived from the
schema at runtime.

**Implementation:** `std/args.mvl::parse_args`

#### Scenario: Help exits cleanly

- GIVEN any schema passed to `parse_args`
- WHEN invoked as `prog -h` or `prog --help`
- THEN the program MUST print the usage string to stdout and exit with code 0

#### Scenario: Usage lists all fields

- GIVEN schema `[positional("file", ArgType::Str), flag("verbose"), optional("port", ArgType::Int)]`
- WHEN `-h` is passed
- THEN the usage string MUST mention `<file>`, `[--verbose]`, and `[--port <Int>]`

---

### Requirement 6: Type coercion [MUST]

`parse_args` MUST coerce raw string tokens to the `ArgType` declared in the
schema.  `Str` is a no-op.  `Int` and `Float` MUST be parsed via `str_parse_int`
and `str_parse_float`.  Coercion failure MUST trigger Requirement 4 (exit).

**Implementation:** `std/args.mvl::coerce_arg`

#### Scenario: Int coercion

- GIVEN schema `[required("port", ArgType::Int)]`
- WHEN invoked as `prog --port 8080`
- THEN `args.get("port")` MUST equal `Some(ArgValue::Int(8080))`

#### Scenario: Float coercion

- GIVEN schema `[required("ratio", ArgType::Float)]`
- WHEN invoked as `prog --ratio 0.5`
- THEN `args.get("ratio")` MUST equal `Some(ArgValue::Float(0.5))`
