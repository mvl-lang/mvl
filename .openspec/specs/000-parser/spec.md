---
domain: compiler
version: 0.1.0
status: draft
date: 2026-04-11
---

# 000 — Parser

The MVL parser transforms source text into an Abstract Syntax Tree (AST). It is a hand-written recursive descent LL(1) parser (ADR-0005) implementing the EBNF grammar defined in [`mvl-spec/grammar/grammar.ebnf`](https://github.com/mvl-lang/mvl-spec/blob/main/grammar/grammar.ebnf).

## Philosophy

The parser is the compiler's front door. Every error message a developer (or LLM) sees comes from here. Quality of diagnostics matters more than parsing speed. The grammar is deliberately LL(1) — one token of lookahead, no ambiguity, no backtracking.

## Requirements

### Requirement 1: Lexer — Tokenization [MUST]

The lexer MUST tokenize MVL source into a stream of typed tokens. Each token MUST carry a source location (file, line, column, byte offset). Keywords MUST be recognized by table lookup after identifier scan.

**Implementation:** `src/mvl/parser/lexer.rs`

**Tests:** inline `#[cfg(test)]` in `src/mvl/parser/lexer.rs`

#### Scenario: Tokenize keywords

- GIVEN source text `fn let mut match if else for type module total partial return`
- WHEN the lexer tokenizes it
- THEN it MUST produce 12 keyword tokens with correct types

**Corpus:** `tests/corpus/01_syntax/keywords.mvl`

#### Scenario: Tokenize operators

- GIVEN source text `+ - * / % == != < > <= >= && || ! ? . :: -> =`
- WHEN the lexer tokenizes it
- THEN it MUST produce tokens for each operator

#### Scenario: Tokenize security labels

- GIVEN source text `Public Tainted Secret Clean iso val ref tag`
- WHEN the lexer tokenizes it
- THEN it MUST produce 8 keyword tokens (security labels and capabilities)

#### Scenario: Tokenize literals

- GIVEN source text `42 3.14 "hello" 'c' true false`
- WHEN the lexer tokenizes it
- THEN it MUST produce INTEGER, FLOAT, STRING, CHAR, BOOL, BOOL tokens

#### Scenario: Source locations

- GIVEN a multi-line source file
- WHEN tokenized
- THEN every token MUST carry correct line and column numbers

**Corpus:** `tests/corpus/01_syntax/literals.mvl`

### Requirement 2: AST Data Structures [MUST]

The parser MUST produce a typed AST using Rust enums and structs. All AST nodes MUST carry source location (Span). The AST MUST represent all MVL constructs without loss of information.

**Implementation:** `src/mvl/parser/ast.rs`

**Tests:** inline `#[cfg(test)]` in `src/mvl/parser/ast.rs`

#### Scenario: AST node for function declaration

- GIVEN a parsed `total fn add(a: Int, b: Int) -> Int { a + b }`
- THEN the AST MUST contain: totality=Total, name="add", params=[a:Int, b:Int], return_type=Int, effects=[], body=[BinaryExpr(+)]

#### Scenario: AST node with security labels

- GIVEN a parsed `fn f(x: Tainted[String]) -> Clean[String]`
- THEN the param type MUST be `LabeledType(Tainted, String)` and return type MUST be `LabeledType(Clean, String)`

### Requirement 3: Parse Type Declarations [MUST]

The parser MUST parse struct, enum, type alias, and refinement type declarations per the EBNF grammar.

**Implementation:** `src/mvl/parser/types.rs`

**Tests:** inline `#[cfg(test)]` in `src/mvl/parser/types.rs`

#### Scenario: Parse struct

- GIVEN `type Point = struct { x: Float64, y: Float64 }`
- WHEN parsed
- THEN AST contains StructDecl with name="Point" and two fields

#### Scenario: Parse enum

- GIVEN `type Result[T, E] = enum { Ok(T), Err(E) }`
- WHEN parsed
- THEN AST contains EnumDecl with name="Result", type_params=[T,E], variants=[Ok(T), Err(E)]

#### Scenario: Parse refinement type

- GIVEN `type PositiveInt = Int where self > 0`
- WHEN parsed
- THEN AST contains AliasDecl with refinement predicate `self > 0`

**Corpus:** `tests/corpus/03_types/structs.mvl`, `tests/corpus/03_types/enums.mvl`, `tests/corpus/03_types/refinements.mvl`

### Requirement 4: Parse Function Declarations [MUST]

The parser MUST parse function declarations including totality annotation, parameters with capabilities and security labels, return types with refinements, and effect lists.

**Implementation:** `src/mvl/parser/functions.rs`

**Tests:** inline `#[cfg(test)]` in `src/mvl/parser/functions.rs`

#### Scenario: Parse total function with effects

- GIVEN `total fn read_file(path: Path) -> Result[String, IOError] ! FileRead { ... }`
- WHEN parsed
- THEN AST contains FnDecl with totality=Total, effects=[FileRead], return_type=Result[String, IOError]

#### Scenario: Parse function with capability parameter

- GIVEN `fn process(iso db: val DbConn) -> Result[Data, Error] ! DB { ... }`
- WHEN parsed
- THEN parameter has capability=Iso, type=Ref(DbConn)

#### Scenario: Parse function with security-labeled params

- GIVEN `fn handle(input: Tainted[String], key: Secret[ApiKey]) -> Public[Response]`
- WHEN parsed
- THEN params have correct security labels, return has Public label

**Corpus:** `tests/corpus/02_functions/functions.mvl`, `tests/corpus/07_effects/declarations.mvl`

### Requirement 5: Parse Statements [MUST]

The parser MUST parse all MVL statement forms: let bindings (with mandatory type annotation; capability prefix `ref` makes a binding mutable), assignment, return, if/else, match, for, and expression statements. A `let` binding without an explicit `: Type` annotation MUST be rejected with a parse error (#408).

**Implementation:** `src/mvl/parser/statements.rs`

**Tests:** inline `#[cfg(test)]` in `src/mvl/parser/statements.rs`

#### Scenario: Parse let with type annotation

- GIVEN `let x: Int = 42;`
- WHEN parsed
- THEN AST contains LetStmt with mutable=false, name="x", type=Some(Int), value=Literal(42)

#### Scenario: Parse mutable let

- GIVEN `let count: ref Int = 0;`
- WHEN parsed
- THEN AST contains LetStmt with type=Some(ref Int), value=Literal(0)

#### Scenario: Parse exhaustive match

- GIVEN `match option { Some(v) => use(v), None => default() }`
- WHEN parsed
- THEN AST contains MatchExpr with two arms covering Some and None

**Corpus:** `tests/corpus/01_syntax/statements.mvl`

### Requirement 6: Parse Expressions [MUST]

The parser MUST parse all MVL expression forms: literals, identifiers, field access, function/method calls, binary operators (numeric only), if-expressions, match-expressions, `?` propagation, `move`, `consume`, `declassify`, and `sanitize`.

**Implementation:** `src/mvl/parser/expressions.rs`

**Tests:** inline `#[cfg(test)]` in `src/mvl/parser/expressions.rs`

#### Scenario: Parse ? propagation

- GIVEN `parse_int(input)?`
- WHEN parsed
- THEN AST contains PropagateExpr wrapping a CallExpr

#### Scenario: Parse sanitize

- GIVEN `sanitize(user_input)`
- WHEN parsed
- THEN AST contains SanitizeExpr wrapping an identifier

#### Scenario: Parse if-expression

- GIVEN `if valid { ok_value } else { err_value }`
- WHEN parsed
- THEN AST contains IfExpr with both branches (it's an expression, not a statement)

#### Scenario: Parse typed-receiver static call

- GIVEN `Map[String, Int]::new()`
- WHEN parsed
- THEN AST contains FnCall with name="Map::new" and type_args=[String, Int]
- AND the call is valid inline (no surrounding let annotation required)

**Corpus:** `tests/corpus/05_collections/map_typed_new.mvl`

**Corpus:** `tests/corpus/01_syntax/expressions.mvl`

### Requirement 7: Parse Security Labels [MUST]

The parser MUST parse security-labeled types (`Public[T]`, `Tainted[T]`, `Secret[T]`, `Clean[T]`) and capability annotations (`iso`, `val`, `ref`, `tag`) as first-class type constructs.

**Implementation:** `src/mvl/parser/types.rs`

**Tests:** inline `#[cfg(test)]` in `src/mvl/parser/types.rs`

#### Scenario: Parse labeled type

- GIVEN type annotation `Tainted[String]`
- WHEN parsed
- THEN AST contains LabeledType with label=Tainted, inner=String

#### Scenario: Parse nested labels

- GIVEN type annotation `Public[Option[Secret[Key]]]`
- WHEN parsed
- THEN AST contains nested LabeledType → OptionType → LabeledType

**Corpus:** `tests/corpus/08_ifc/labels.mvl`

### Requirement 8-SH: Self-Hosted Parser — Alternative Implementation [MUST]

The MVL parser SHALL have a self-hosted implementation written in MVL itself
(`compiler/parser.mvl`) that produces an AST equivalent to the Rust reference
implementation for all corpus programs (#1116).

**Primary implementation:** `src/mvl/parser/` (Rust, reference)
**Self-hosted implementation:** `compiler/parser.mvl` + `compiler/tir.mvl`

The self-hosted parser MUST satisfy cross-validation requirements:
- It SHALL tokenize all corpus programs identically to `compiler/lexer.mvl`.
- It SHALL produce a structurally equivalent AST for all corpus programs.
- It SHALL parse its own source file without errors (self-referential test).
- Error spans MUST match (same line/column as Rust parser).

**Shared type definitions:** `compiler/tir.mvl` — single source of truth for
`Token`, `TokenKind`, `Expr`, `Stmt`, `Block`, `Pattern`, `FnDecl`, and all
AST node types used by both the parser and downstream stages.

**Tests:** `compiler/parser_test.mvl` (Phase 3 section)

#### Scenario: Self-hosted body parsing

- GIVEN a function with a body `fn add(a: Int, b: Int) -> Int { a + b }`
- WHEN parsed by `compiler/parser.mvl`
- THEN `FnDecl.body` MUST be `Some(block)` where block contains a BinaryExpr tail

#### Scenario: Self-referential parse

- GIVEN the file `compiler/parser.mvl` itself as input
- WHEN parsed by `compiler/parser.mvl`
- THEN all functions MUST be parsed without errors (body = Some(…))

#### Scenario: Cross-validation

- GIVEN any corpus `.mvl` file
- WHEN parsed by both the Rust parser and `compiler/parser.mvl`
- THEN the declaration counts (fns, types, uses, consts) MUST match

### Requirement 8: Error Recovery and Diagnostics [MUST]

The parser MUST recover from errors and report multiple diagnostics per file. Every diagnostic MUST include file path, line number, column number, and a human-readable message. The parser SHOULD suggest fixes where possible.

**Implementation:** `src/mvl/parser/diagnostics.rs`

**Tests:** `tests/integration/error_messages/`

#### Scenario: Multiple errors reported

- GIVEN a source file with three syntax errors
- WHEN parsed
- THEN the parser MUST report all three errors, not just the first

#### Scenario: Error with source location

- GIVEN a missing `}` on line 12 column 5
- WHEN the parser encounters it
- THEN the diagnostic MUST read: "expected `}` to close block started at line 8:3, found EOF at line 12:5"

#### Scenario: Recovery after error

- GIVEN `fn broken( { }` followed by `fn valid() -> Int { 42 }`
- WHEN parsed
- THEN `broken` produces an error AND `valid` parses successfully
