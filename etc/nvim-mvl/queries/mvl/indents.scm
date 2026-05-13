; MVL indentation queries for nvim-treesitter
; Controls automatic indentation when pressing Enter.

; Indent inside blocks
(block) @indent

; Indent inside struct and enum bodies
(struct_body) @indent
(enum_body) @indent

; Indent inside match arms
(match_stmt) @indent
(match_expr) @indent

; Indent after opening brackets
[
  "{"
  "("
  "["
] @indent

; De-indent on closing brackets
[
  "}"
  ")"
  "]"
] @dedent

; Branches that indent one level
(if_stmt "else" @branch)
(match_arm "=>" @branch)
