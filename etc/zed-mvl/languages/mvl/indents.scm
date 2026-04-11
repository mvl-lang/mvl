; MVL indentation queries for Zed

; Indent inside braced blocks
(block "{" @start "}" @end) @indent

; Indent inside struct and enum bodies
(struct_body "{" @start "}" @end) @indent
(enum_body "{" @start "}" @end) @indent

; Indent inside module declarations
(module_decl "{" @start "}" @end) @indent

; Indent inside parenthesised parameter lists (multi-line function signatures)
(fn_decl "(" @start ")" @end) @indent
(param_list) @indent

; De-indent closing delimiters to align with opening line
"}" @dedent
")" @dedent
"]" @dedent
