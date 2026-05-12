; MVL Tree-sitter highlight queries
; Compatible with Zed (zed-extensions) and Neovim (nvim-treesitter)

; ============================================================
; Comments
; ============================================================

(line_comment) @comment

; ============================================================
; Keywords — declarations
; ============================================================

[
  "type"
  "fn"
  "const"
  "struct"
  "enum"
] @keyword

[
  "use"
  "pub"
] @keyword.import

; ============================================================
; Keywords — totality (safety modifiers)
; ============================================================

(totality) @keyword.modifier

; ============================================================
; Keywords — security modifiers on functions
; ============================================================

(security_modifier) @keyword.modifier

; ============================================================
; Keywords — statements
; ============================================================

[
  "let"
  "return"
  "if"
  "else"
  "match"
  "for"
  "in"
  "while"
  "where"
] @keyword

; ============================================================
; Keywords — expressions
; ============================================================

[
  "consume"
] @keyword.operator

(declassify_expr "declassify" @keyword.special)
(sanitize_expr "sanitize" @keyword.special)

; ============================================================
; Capability annotations (ownership/isolation)
; ============================================================

(capability) @keyword.modifier

; ============================================================
; Security labels — type-level information flow control
; ============================================================

(security_label) @type.qualifier

; ============================================================
; Built-in generic types
; ============================================================

(option_type "Option" @type.builtin)
(result_type "Result" @type.builtin)

; ============================================================
; Built-in effects (effect list)
; ============================================================

(effect) @keyword.effect

; ============================================================
; Pattern keywords
; ============================================================

(none_pattern) @constant.builtin
(some_pattern "Some" @constructor)
(ok_pattern "Ok" @constructor)
(err_pattern "Err" @constructor)

"_" @variable.builtin

; ============================================================
; Boolean literals
; ============================================================

(boolean_literal) @constant.builtin

; ============================================================
; Numeric literals
; ============================================================

(integer_literal) @number
(float_literal) @number.float

; ============================================================
; String and character literals
; ============================================================

(string_literal) @string
(char_literal) @character

; ============================================================
; Type declarations
; ============================================================

(type_decl
  (identifier) @type.definition)

(struct_body
  (field_decl
    (identifier) @variable.member))

(enum_body
  (variant
    (identifier) @constructor))

; ============================================================
; Function declarations
; ============================================================

(fn_decl
  (identifier) @function)

(param
  (identifier) @variable.parameter)

(const_decl
  (identifier) @constant)

; ============================================================
; Module imports
; ============================================================

(use_decl
  (module_path) @namespace)

; ============================================================
; Type expressions
; ============================================================

(base_type
  (identifier) @type)

; ============================================================
; Function calls
; ============================================================

(fn_call_expr
  function: (identifier) @function.call)

; ============================================================
; Struct construction
; ============================================================

(construct_expr
  type: (identifier) @constructor)

; ============================================================
; Operators
; ============================================================

[
  "->"
  "=>"
  "="
  "+"
  "-"
  "*"
  "/"
  "%"
  "=="
  "!="
  "<"
  ">"
  "<="
  ">="
  "&&"
  "||"
  "!"
  "&"
  "|"
  "^"
  "~"
  "<<"
  ">>"
] @operator

; ============================================================
; Punctuation
; ============================================================

[
  "("
  ")"
  "{"
  "}"
  "["
  "]"
] @punctuation.bracket

[
  ","
  ";"
  ":"
  "."
] @punctuation.delimiter
