// Tree-sitter grammar for MVL (Minimum Verification Language)
// Derived from docs/grammar.ebnf — LL(1), no parsing ambiguities.
//
// Note on <> vs comparison: `<` only appears in type position in MVL.
// To avoid the classic generic-vs-comparison ambiguity for the syntax
// highlighter, explicit type args at call sites are parsed via separate
// `_typed_call` forms rather than `identifier < type_list > ( args )`.
//
// Precedence levels (higher number = tighter binding):
//   1  OR        ||
//   2  AND       &&
//   3  BITOR     |
//   4  BITXOR    ^
//   5  BITAND    & (binary)
//   6  COMPARE   == != < > <= >=
//   7  SHIFT     << >>
//   8  ADD       + -
//   9  MUL       * / %
//  10  UNARY     ! ~ - move consume  (right-assoc)
//  11  CALL      . ()                (left-assoc — method/field access)
//  12  POSTFIX   ?                   (left-assoc — Result propagation)

const PREC = {
  OR: 1,
  AND: 2,
  BITOR: 3,
  BITXOR: 4,
  BITAND: 5,
  COMPARE: 6,
  SHIFT: 7,
  ADD: 8,
  MUL: 9,
  UNARY: 10,
  CALL: 11,
  POSTFIX: 12,
};

module.exports = grammar({
  name: "mvl",

  extras: ($) => [/\s/, $.line_comment],

  // GLR conflicts: `where` after a type_expr is ambiguous between
  // a refined_type and an outer `where constraints` clause.
  // `construct_expr` (identifier '{') vs block after if/while is also ambiguous.
  conflicts: ($) => [
    // `where` after return type_expr: return-type refinement vs fn-level constraints
    [$.return_type],
    // `!` after fn_type's return type_expr: fn_type effects vs fn_decl effects
    [$.fn_type],
    // match/if as statement vs expression (syntactically identical;
    // context determines which interpretation applies)
    [$.match_stmt, $.match_expr],
    [$.if_stmt, $.if_expr],
    // effect_list: `effect ,` — continue list vs end list (outer comma)
    [$.effect_list],
    // effect: `identifier (` — parameterized effect string vs outer expression
    [$.effect],
    // `>` inside `Public<Int where self > 0>`: could be comparison or close generic.
    // GLR explores both; only the one that closes the generic succeeds.
    [$.ref_expr, $.labeled_type],
    [$.ref_expr, $.base_type],
    [$.ref_expr, $.result_type],
    [$.ref_expr, $.option_type],
  ],

  rules: {
    // === Top-level ===
    // "pub" is factored out of declaration so each decl_body alternative
    // starts with a distinct keyword — preserves LL(1) (mirrors grammar.ebnf).

    program: ($) =>
      seq(repeat($.use_decl), repeat($.declaration)),

    // Two structural forms:
    //   1. optional "pub" + non-import decl body (type/fn/const/extern/impl)
    //   2. reexport_decl — carries its own "pub" to stay unambiguous vs use_decl
    declaration: ($) =>
      choice(
        seq(
          optional("pub"),
          choice(
            $.type_decl,
            $.fn_decl,
            $.const_decl,
            $.extern_decl,
            $.impl_decl
          )
        ),
        $.reexport_decl
      ),

    // === Modules and imports ===
    // One file = one module (file=module, no inline module blocks).
    // Module name = filename without extension.

    // `use path::to::Item;` — private import (top of file only)
    use_decl: ($) => seq("use", $.module_path, ";"),

    // `pub use path::to::Item;` — re-export; "pub" is part of the rule so it
    // remains unambiguous with use_decl (which never has "pub").
    reexport_decl: ($) => seq("pub", "use", $.module_path, ";"),

    module_path: ($) =>
      seq($.identifier, repeat(seq("::", $.identifier))),

    // Impl block: `impl Trait [ TypeParams ] for Type { fn_decls }`
    impl_decl: ($) =>
      seq(
        "impl",
        $.identifier,
        optional(seq("[", $.type_list, "]")),
        "for",
        $.identifier,
        "{",
        repeat($.fn_decl),
        "}"
      ),

    // Extern trust boundary: `extern "rust" { fn foo(...) -> T; }`
    extern_decl: ($) =>
      seq("extern", $.string_literal, "{", repeat($.extern_fn_decl), "}"),

    // Function signature inside an extern block (no body — semicolon instead)
    extern_fn_decl: ($) =>
      seq(
        "fn",
        $.identifier,
        "(",
        optional($.param_list),
        ")",
        "->",
        $.type_expr,
        optional(seq("!", $.effect_list)),
        ";"
      ),

    // === Type declarations ===

    type_decl: ($) =>
      seq("type", $.identifier, optional($.type_params), "=", $.type_body),

    type_params: ($) =>
      seq("<", $.identifier, repeat(seq(",", $.identifier)), ">"),

    type_body: ($) => choice($.struct_body, $.enum_body, $.type_expr),

    struct_body: ($) => seq("struct", "{", repeat($.field_decl), "}"),

    enum_body: ($) =>
      seq(
        "enum",
        "{",
        $.variant,
        repeat(seq(",", $.variant)),
        optional(","),
        "}"
      ),

    variant: ($) =>
      seq(
        $.identifier,
        optional(
          choice(
            seq("(", $.type_list, ")"),
            seq("{", $.field_list, "}")
          )
        )
      ),

    field_decl: ($) =>
      seq(
        optional("mut"),
        $.identifier,
        ":",
        $.type_expr,
        optional(seq("where", $.refinement)),
        optional(",")
      ),

    field_list: ($) => seq($.field_decl, repeat($.field_decl)),

    // === Function declarations ===

    fn_decl: ($) =>
      seq(
        optional(choice($.totality, "builtin")),
        optional($.security_modifier),
        "fn",
        $.identifier,
        optional($.type_params),
        "(",
        optional($.param_list),
        ")",
        "->",
        $.return_type,
        optional(seq("!", $.effect_list)),
        optional(seq("where", $.constraints)),
        optional($.block) // builtin fns have no body; required for non-builtin
      ),

    totality: ($) => choice("total", "partial"),

    security_modifier: ($) => choice("public", "tainted", "secret"),

    param_list: ($) =>
      seq($.param, repeat(seq(",", $.param)), optional(",")),

    param: ($) =>
      seq(
        optional($.capability),
        optional("mut"),
        $.identifier,
        ":",
        $.type_expr,
        optional(seq("where", $.refinement))
      ),

    capability: ($) => choice("iso", "val", "ref", "tag"),

    return_type: ($) =>
      seq($.type_expr, optional(seq("where", $.refinement))),

    effect_list: ($) => seq($.effect, repeat(seq(",", $.effect))),

    effect: ($) =>
      seq(
        /[a-zA-Z_][a-zA-Z0-9_]*/,
        optional(seq("(", $.string_literal, ")"))
      ),

    constraints: ($) => seq($.constraint, repeat(seq(",", $.constraint))),

    constraint: ($) => seq($.identifier, ":", $.trait_bound),

    // Phase 1: single trait bound only; "+" compound bounds deferred to Phase 2
    trait_bound: ($) => $.identifier,

    // === Type expressions ===

    type_expr: ($) =>
      choice(
        $.refined_type,
        $.fn_type,
        $.tuple_type,
        $.option_type,
        $.result_type,
        $.ref_type,
        $.labeled_type,
        $.base_type
      ),

    base_type: ($) =>
      seq($.identifier, optional(seq("<", $.type_list, ">"))),

    option_type: ($) => seq("Option", "<", $.type_expr, ">"),

    result_type: ($) =>
      seq("Result", "<", $.type_expr, ",", $.type_expr, ">"),

    ref_type: ($) => seq(choice("val", "ref"), $.type_expr),

    labeled_type: ($) => seq($.security_label, "<", $.type_expr, ">"),

    security_label: ($) => choice("Public", "Tainted", "Secret", "Clean"),

    refined_type: ($) => prec(1, seq($.base_type, "where", $.refinement)),

    fn_type: ($) =>
      seq(
        "fn",
        "(",
        optional($.type_list),
        ")",
        "->",
        $.type_expr,
        optional(seq("!", $.effect_list))
      ),

    tuple_type: ($) => seq("(", $.type_expr, ",", $.type_list, ")"),

    type_list: ($) => seq($.type_expr, repeat(seq(",", $.type_expr))),

    // === Refinement predicates ===
    // Structured with precedence to avoid left-recursion conflicts.

    refinement: ($) => $.ref_expr,

    ref_expr: ($) =>
      choice(
        prec.left(1, seq($.ref_expr, "||", $.ref_expr)),
        prec.left(2, seq($.ref_expr, "&&", $.ref_expr)),
        prec.left(
          3,
          seq(
            $.ref_expr,
            choice("==", "!=", "<", ">", "<=", ">="),
            $.ref_expr
          )
        ),
        prec.left(4, seq($.ref_expr, choice("+", "-"), $.ref_expr)),
        prec.left(5, seq($.ref_expr, choice("*", "/", "%"), $.ref_expr)),
        prec.right(6, seq("!", $.ref_expr)),
        $.ref_atom
      ),

    ref_term: ($) => $.ref_expr,

    ref_atom: ($) =>
      choice(
        seq("len", "(", $.identifier, ")"),
        seq("(", $.ref_expr, ")"),
        $.identifier,
        $.integer_literal,
        $.float_literal
      ),

    // === Statements ===

    // Blocks may end with an implicit return expression (no semicolon),
    // similar to Rust. This is the final expression value of the block.
    block: ($) =>
      seq("{", repeat($.statement), optional($.expr), "}"),

    statement: ($) =>
      choice(
        $.let_stmt,
        $.assign_stmt,
        $.return_stmt,
        $.if_stmt,
        $.match_stmt,
        $.for_stmt,
        $.while_stmt,
        $.expr_stmt
      ),

    let_stmt: ($) =>
      seq(
        "let",
        optional("mut"),
        $.pattern,
        seq(":", $.type_expr),
        "=",
        $.expr,
        ";"
      ),

    // Use expr as lvalue — `=` vs `==` are different tokens, no conflict.
    assign_stmt: ($) => prec(1, seq($.expr, "=", $.expr, ";")),

    return_stmt: ($) => seq("return", optional($.expr), ";"),

    if_stmt: ($) =>
      seq(
        "if",
        $.expr,
        $.block,
        optional(seq("else", choice($.if_stmt, $.block)))
      ),

    match_stmt: ($) =>
      seq("match", $.expr, "{", repeat($.match_arm), "}"),

    match_arm: ($) =>
      seq(
        $.pattern,
        optional(seq("where", $.refinement)),
        "=>",
        $.expr,
        optional(",")
      ),

    for_stmt: ($) => seq("for", $.pattern, "in", $.expr, $.block),

    while_stmt: ($) => seq("while", $.expr, $.block),

    expr_stmt: ($) => seq($.expr, ";"),

    // === Expressions ===
    // All left-recursive operations (field access, method calls, propagation)
    // are inlined into `expr` with `prec.left` — the standard tree-sitter pattern.

    expr: ($) =>
      choice(
        // Postfix — result/option propagation with `?`
        prec.left(
          PREC.POSTFIX,
          seq(field("operand", $.expr), "?")
        ),
        // Member access: method call (must come before field access — more specific)
        prec.left(
          PREC.CALL,
          seq(
            field("object", $.expr),
            ".",
            field("method", $.identifier),
            "(",
            optional(field("arguments", $.arg_list)),
            ")"
          )
        ),
        // Member access: field read
        prec.left(
          PREC.CALL,
          seq(
            field("object", $.expr),
            ".",
            field("field", $.identifier)
          )
        ),
        // Unary operators (right-associative)
        prec.right(PREC.UNARY, seq("!", $.expr)),
        prec.right(PREC.UNARY, seq("~", $.expr)),
        prec.right(PREC.UNARY, seq("-", $.expr)),
        prec.right(PREC.UNARY, seq("move", $.expr)),
        prec.right(PREC.UNARY, seq("consume", $.expr)),
        // Binary operators
        prec.left(PREC.MUL, seq($.expr, choice("*", "/", "%"), $.expr)),
        prec.left(PREC.ADD, seq($.expr, choice("+", "-"), $.expr)),
        prec.left(PREC.SHIFT, seq($.expr, choice("<<", ">>"), $.expr)),
        prec.left(
          PREC.COMPARE,
          seq($.expr, choice("==", "!=", "<", ">", "<=", ">="), $.expr)
        ),
        prec.left(PREC.BITAND, seq($.expr, "&", $.expr)),
        prec.left(PREC.BITXOR, seq($.expr, alias("^", $.bitxor_op), $.expr)),
        prec.left(PREC.BITOR, seq($.expr, "|", $.expr)),
        prec.left(PREC.AND, seq($.expr, "&&", $.expr)),
        prec.left(PREC.OR, seq($.expr, "||", $.expr)),
        // Atoms
        $._atom_expr
      ),

    // Borrow expression: `val expr` or `ref expr`
    borrow_expr: ($) =>
      prec.right(PREC.UNARY, seq(choice("val", "ref"), $.expr)),

    // Atomic (non-recursive) expression forms
    _atom_expr: ($) =>
      choice(
        $.literal,
        $.if_expr,
        $.match_expr,
        $.lambda_expr,
        $.block_expr,
        $.declassify_expr,
        $.sanitize_expr,
        $.construct_expr,
        $.fn_call_expr,
        $.grouped_expr,
        $.path_expr,
        $.borrow_expr,
        $.identifier
      ),

    // Path expression: Enum::Variant or Module::function
    // Used for enum variant access (e.g. AuthError::NotFound).
    path_expr: ($) =>
      prec.left(
        PREC.CALL + 1,
        seq($.identifier, "::", $.identifier)
      ),

    // Note: explicit type arguments at call sites (e.g. foo<T>(x)) are omitted
    // to avoid the classic `<` ambiguity (generic vs comparison operator).
    // MVL type arguments at call sites are always inferred in practice.
    // prec(1): prefer fn_call_expr over bare identifier when `(` follows.
    fn_call_expr: ($) =>
      prec(1, seq(
        field("function", $.identifier),
        "(",
        optional(field("arguments", $.arg_list)),
        ")"
      )),

    arg_list: ($) =>
      seq($.expr, repeat(seq(",", $.expr)), optional(",")),

    lambda_expr: ($) =>
      seq(
        "|",
        optional($.param_list),
        "|",
        optional(seq("->", $.type_expr)),
        $.expr
      ),

    construct_expr: ($) =>
      prec(
        -1,
        seq(
          field("type", $.identifier),
          "{",
          repeat(seq(field("field", $.identifier), ":", $.expr, ",")),
          "}"
        )
      ),

    if_expr: ($) =>
      seq("if", $.expr, $.block, "else", $.block),

    match_expr: ($) =>
      seq("match", $.expr, "{", repeat($.match_arm), "}"),

    block_expr: ($) => $.block,

    grouped_expr: ($) => seq("(", $.expr, ")"),

    declassify_expr: ($) => seq("declassify", "(", $.expr, ")"),

    sanitize_expr: ($) => seq("sanitize", "(", $.expr, ")"),

    // === Patterns ===

    pattern: ($) =>
      choice(
        "_",
        $.literal,
        $.tuple_pattern,
        $.some_pattern,
        $.none_pattern,
        $.ok_pattern,
        $.err_pattern,
        $.constructor_pattern,
        $.struct_pattern,
        $.identifier
      ),

    constructor_pattern: ($) =>
      seq($.identifier, "(", optional($.pattern_list), ")"),

    struct_pattern: ($) =>
      seq(
        $.identifier,
        "{",
        repeat(seq($.identifier, ":", $.pattern, ",")),
        "}"
      ),

    tuple_pattern: ($) =>
      seq("(", $.pattern, ",", $.pattern_list, ")"),

    some_pattern: ($) => seq("Some", "(", $.pattern, ")"),

    none_pattern: ($) => "None",

    ok_pattern: ($) => seq("Ok", "(", $.pattern, ")"),

    err_pattern: ($) => seq("Err", "(", $.pattern, ")"),

    pattern_list: ($) => seq($.pattern, repeat(seq(",", $.pattern))),

    // === Literals ===

    literal: ($) =>
      choice(
        $.integer_literal,
        $.float_literal,
        $.string_literal,
        $.multiline_string_literal,
        $.raw_string_literal,
        $.raw_multiline_string_literal,
        $.char_literal,
        $.boolean_literal,
        $.list_literal,
        $.map_literal,
        $.set_literal
      ),

    integer_literal: ($) => /[0-9]+/,

    // Float must be tried before integer (both start with digits).
    float_literal: ($) => /[0-9]+\.[0-9]+/,

    string_literal: ($) =>
      seq('"', repeat(choice(/[^"\\]/, /\\./)), '"'),

    // `"""…"""` — multiline string (escape sequences processed).
    // Uses token() to avoid lookahead: content is any run of non-quote/non-backslash
    // chars or escape sequences, with 1- or 2-quote runs allowed when followed by
    // more content, plus an optional trailing 1 or 2 quotes before the `"""` close.
    multiline_string_literal: (_) =>
      token(seq(
        '"""',
        /([^"\\]|\\.)*("([^"\\]|\\.)+|""([^"\\]|\\.)+)*(""|")?/,
        '"""',
      )),

    // `r"…"` — raw single-line string (no escape processing).
    raw_string_literal: ($) =>
      seq('r"', repeat(/[^"]/), '"'),

    // `r"""…"""` — raw multiline string (no escape processing).
    // Uses token() to avoid lookahead: same approach as multiline_string_literal
    // but without escape sequences.
    raw_multiline_string_literal: (_) =>
      token(seq(
        'r"""',
        /[^"]*("[^"]+|""[^"]+)*(""|")?/,
        '"""',
      )),

    char_literal: ($) =>
      seq("'", choice(/[^'\\]/, /\\./), "'"),

    boolean_literal: ($) => choice("true", "false"),

    list_literal: ($) =>
      seq("[", optional(seq($.expr, repeat(seq(",", $.expr)))), "]"),

    // `{"k": v, …}` — map literal. Disambiguation: colon after first expression.
    map_literal: ($) =>
      seq(
        "{",
        $.expr, ":", $.expr,
        repeat(seq(",", $.expr, ":", $.expr)),
        optional(","),
        "}"
      ),

    // `{"a", "b", …}` — set literal. Two+ elements or trailing comma required
    // to disambiguate from block expressions.
    set_literal: ($) =>
      choice(
        // Two or more elements
        seq("{", $.expr, ",", $.expr, repeat(seq(",", $.expr)), optional(","), "}"),
        // Single element with trailing comma
        seq("{", $.expr, ",", "}")
      ),

    // === Constants ===

    const_decl: ($) =>
      seq("const", $.identifier, ":", $.type_expr, "=", $.expr, ";"),

    // === Lexical ===

    identifier: ($) => /[a-zA-Z_][a-zA-Z0-9_]*/,

    line_comment: ($) => token(seq("//", /.*/)),
  },
});
