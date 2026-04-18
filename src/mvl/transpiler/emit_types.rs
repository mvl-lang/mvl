//! Emit Rust type declarations from MVL type declarations.
//!
//! Mappings:
//! - `type Foo = struct { … }` → `pub struct Foo { … }`
//! - `type Bar = enum { … }` → `pub enum Bar { … }`
//! - `type Alias = T` → `pub type Alias = <rust_type>;`
//! - `type Refined = T where pred` → newtype with constructor validation
//! - Security labels (Public<T> etc.) → module-level preamble structs
//! - Refinement field predicates → `debug_assert!` in constructors
//! - Concrete structs with parseable fields → `impl ParseFromArgs`

use crate::mvl::parser::ast::{
    FieldDecl, GenericParam, RefExpr, SecurityLabel, TypeBody, TypeDecl, TypeExpr, Variant,
    VariantFields,
};
use crate::mvl::transpiler::codegen::Codegen;

// ── Security label preamble ───────────────────────────────────────────────

/// Emit the security-label newtype wrappers that every MVL program needs.
///
/// ```rust
/// #[derive(Debug, Clone, PartialEq)]
/// pub struct Public<T>(pub T);
/// // … etc.
/// ```
///
/// Phase 1: only `From`/`Into` for upward lattice flows are generated.
/// The lattice is: Tainted → Clean → Public (least trusted to most trusted).
/// Secret is a separate confidentiality label; only explicit declassify/sanitize
/// can convert between them.
pub fn emit_security_preamble(cg: &mut Codegen) {
    cg.line("// ── Security label newtypes (MVL Req 11) ─────────────────────────────────");
    cg.blank();

    for label in ["Public", "Tainted", "Secret", "Clean"] {
        emit_label_newtype(cg, label);
        cg.blank();
    }

    // Lattice flows: Tainted → Clean (after sanitize)
    // We express this as a From impl: Clean<T>: From<Tainted<T>> is NOT emitted
    // because sanitize() is an explicit conversion; the Rust type system enforces
    // that you must call sanitize() / declassify() explicitly.
    //
    // Phase 1: conversion functions emitted as standalone fns.
    cg.line("/// Sanitize a tainted value — cleans external input.");
    cg.line("/// MVL: `sanitize(x)` where x: Tainted<T>");
    cg.line("pub fn sanitize<T>(v: Tainted<T>) -> Clean<T> { Clean(v.0) }");
    cg.blank();
    cg.line("/// Declassify a secret value — makes it public.");
    cg.line("/// MVL: `declassify(x)` where x: Secret<T>");
    cg.line("pub fn declassify<T>(v: Secret<T>) -> Public<T> { Public(v.0) }");
    cg.blank();
    // Numeric conversion helpers for labeled integer/float types
    cg.line("impl Public<i64> {");
    cg.push_indent();
    cg.line("/// Convert labeled integer to raw f64 (for use with Float-typed functions).");
    cg.line("pub fn to_float(&self) -> f64 { self.0 as f64 }");
    cg.pop_indent();
    cg.line("}");
    cg.blank();
    // ── Higher-order method traits ─────────────────────────────────────────
    // MvlMap: uniform `.map(f)` across Vec<T>, Option<T>, Result<T,E>
    cg.line("pub trait MvlMap { type Inner; type Mapped<U>; fn mvl_map<U, F: FnMut(Self::Inner) -> U>(self, f: F) -> Self::Mapped<U>; }");
    cg.line("impl<T> MvlMap for Vec<T> { type Inner = T; type Mapped<U> = Vec<U>; fn mvl_map<U, F: FnMut(T) -> U>(self, mut f: F) -> Vec<U> { self.into_iter().map(|x| f(x)).collect() } }");
    cg.line("impl<T> MvlMap for Option<T> { type Inner = T; type Mapped<U> = Option<U>; fn mvl_map<U, F: FnMut(T) -> U>(self, mut f: F) -> Option<U> { self.map(|x| f(x)) } }");
    cg.line("impl<T, E> MvlMap for Result<T, E> { type Inner = T; type Mapped<U> = Result<U, E>; fn mvl_map<U, F: FnMut(T) -> U>(self, mut f: F) -> Result<U, E> { self.map(|x| f(x)) } }");
    cg.blank();
    // MvlPow: uniform `.pow(e)` for i64 and f64
    cg.line("pub trait MvlPow { fn mvl_pow(self, exp: Self) -> Self; }");
    cg.line("impl MvlPow for i64 { fn mvl_pow(self, exp: i64) -> i64 { self.pow(exp as u32) } }");
    cg.line("impl MvlPow for f64 { fn mvl_pow(self, exp: f64) -> f64 { self.powf(exp) } }");
}

fn emit_label_newtype(cg: &mut Codegen, label: &str) {
    cg.line("#[derive(Debug, Clone, PartialEq)]");
    cg.line(&format!("pub struct {label}<T>(pub T);"));
    cg.blank();
    // Copy impl: labels over Copy types are themselves Copy (e.g. Public<i64>)
    cg.line(&format!("impl<T: Copy> Copy for {label}<T> {{}}"));
    cg.blank();
    cg.line(&format!("impl<T> {label}<T> {{"));
    cg.push_indent();
    cg.line("pub fn new(v: T) -> Self { Self(v) }");
    cg.line("pub fn into_inner(self) -> T { self.0 }");
    cg.line("pub fn as_inner(&self) -> &T { &self.0 }");
    cg.pop_indent();
    cg.line("}");
    cg.blank();
    // as_str(): enables `match labeled_string.as_str() { "foo" => ... }` in generated code
    cg.line(&format!(
        "impl {label}<String> {{ pub fn as_str(&self) -> &str {{ self.0.as_str() }} }}"
    ));
    cg.blank();
    // Display: label<T> displays as T when T: Display
    cg.line(&format!(
        "impl<T: std::fmt::Display> std::fmt::Display for {label}<T> {{"
    ));
    cg.push_indent();
    cg.line("fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.0.fmt(f) }");
    cg.pop_indent();
    cg.line("}");
    cg.blank();
    // Arithmetic: delegate ops to the inner value, preserving the label
    for (trait_name, method, op) in [
        ("std::ops::Add", "add", "+"),
        ("std::ops::Sub", "sub", "-"),
        ("std::ops::Mul", "mul", "*"),
        ("std::ops::Div", "div", "/"),
        ("std::ops::Rem", "rem", "%"),
    ] {
        cg.line(&format!(
            "impl<T: {trait_name}<Output=T>> {trait_name} for {label}<T> {{"
        ));
        cg.push_indent();
        cg.line(&format!("type Output = {label}<T>;"));
        cg.line(&format!(
            "fn {method}(self, rhs: Self) -> Self {{ {label}(self.0 {op} rhs.0) }}"
        ));
        cg.pop_indent();
        cg.line("}");
        cg.blank();
    }
    cg.line(&format!(
        "impl<T: std::ops::Neg<Output=T>> std::ops::Neg for {label}<T> {{"
    ));
    cg.push_indent();
    cg.line(&format!("type Output = {label}<T>;"));
    cg.line(&format!("fn neg(self) -> Self {{ {label}(-self.0) }}"));
    cg.pop_indent();
    cg.line("}");
}

// ── TypeDecl ─────────────────────────────────────────────────────────────

pub fn emit_type_decl(cg: &mut Codegen, td: &TypeDecl) {
    match &td.body {
        TypeBody::Struct(fields) => {
            emit_struct(cg, &td.name, &td.params, fields);
            // Emit ParseFromArgs impl for concrete (non-generic) structs, but
            // only when the program uses mvl_runtime (ParseFromArgs, get_arg
            // are defined there). Programs without stdlib imports use an inline
            // preamble that does not include these symbols.
            if td.params.is_empty() && cg.use_mvl_runtime {
                emit_parse_from_args_impl(cg, &td.name, fields);
            }
        }
        TypeBody::Enum(variants) => emit_enum(cg, &td.name, &td.params, variants),
        TypeBody::Alias(ty) => emit_alias(cg, &td.name, &td.params, ty),
    }
}

// ── Struct ────────────────────────────────────────────────────────────────

fn emit_struct(cg: &mut Codegen, name: &str, params: &[GenericParam], fields: &[FieldDecl]) {
    emit_derive(cg, &["Debug", "Clone", "PartialEq"]);
    cg.line(&format!("pub struct {}{} {{", name, generic_params(params)));
    cg.push_indent();
    for field in fields {
        let ty_str = emit_type_expr(&field.ty);
        cg.line(&format!("pub {}: {},", field.name, ty_str));
    }
    cg.pop_indent();
    cg.line("}");

    // Emit a constructor if any field has a refinement predicate
    let refined_fields: Vec<_> = fields.iter().filter(|f| f.refinement.is_some()).collect();
    if !refined_fields.is_empty() {
        cg.blank();
        cg.line(&format!(
            "impl{} {}{} {{",
            generic_params(params),
            name,
            generic_params(params)
        ));
        cg.push_indent();
        cg.line(&format!(
            "/// Construct `{}`, validating all refinement predicates.",
            name
        ));
        let param_list: Vec<String> = fields
            .iter()
            .map(|f| format!("{}: {}", f.name, emit_type_expr(&f.ty)))
            .collect();
        cg.line(&format!("pub fn new({}) -> Self {{", param_list.join(", ")));
        cg.push_indent();
        for field in &refined_fields {
            if let Some(pred) = &field.refinement {
                let pred_str = emit_ref_expr_for_assert(pred, &field.name);
                cg.line(&format!(
                    "debug_assert!({pred_str}, \"refinement violated: {} {{}}\", {});",
                    field.name, field.name
                ));
            }
        }
        let field_inits: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
        cg.line(&format!("Self {{ {} }}", field_inits.join(", ")));
        cg.pop_indent();
        cg.line("}");
        cg.pop_indent();
        cg.line("}");
    }
}

// ── ParseFromArgs impl ────────────────────────────────────────────────────

/// Emit `impl ParseFromArgs for StructName` for concrete structs whose fields
/// all have parseable types.
///
/// Parseable field types: `Int`, `Float`, `String`, `Bool`, `Option<Int>`,
/// `Option<Float>`, `Option<String>`, and refined variants of the above.
///
/// Skipped when any field has an unsupported type (e.g. nested structs,
/// generic params, security labels) — callers receive a Rust type-error if
/// they attempt `parse::<T>()` on such a struct.
pub(crate) fn emit_parse_from_args_impl(cg: &mut Codegen, name: &str, fields: &[FieldDecl]) {
    // Only emit for structs where every field is parseable
    if !fields.iter().all(|f| is_parseable_field_type(&f.ty)) {
        return;
    }
    cg.blank();
    cg.line(&format!("impl ParseFromArgs for {} {{", name));
    cg.push_indent();
    cg.line("fn parse_from_args() -> Result<Self, String> {");
    cg.push_indent();

    for field in fields {
        emit_field_parse(cg, field);
    }

    let field_names: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
    cg.line(&format!("Ok(Self {{ {} }})", field_names.join(", ")));
    cg.pop_indent();
    cg.line("}");
    cg.pop_indent();
    cg.line("}");
}

/// Emit the parsing code for a single struct field.
///
/// Field names must be valid Rust identifiers — asserted by
/// `assert_safe_identifier` before any interpolation into generated code.
fn emit_field_parse(cg: &mut Codegen, field: &FieldDecl) {
    let name = &field.name;
    // The CLI flag name is the field name directly: `port` → `--port`.
    assert_safe_identifier(name);

    // Unwrap any refinement wrapper to get the base type for parsing
    let base_ty = unwrap_refined_ty(&field.ty);

    match base_ty {
        // Bool → flag presence (no value argument)
        TypeExpr::Base {
            name: ty_name,
            args,
            ..
        } if ty_name == "Bool" && args.is_empty() => {
            cg.line(&format!(
                "let {name} = std::env::args().any(|__a| __a == \"--{name}\");"
            ));
        }
        // Option<String> → optional string flag
        TypeExpr::Option { inner, .. } if matches!(unwrap_refined_ty(inner), TypeExpr::Base { name: n, args, .. } if n == "String" && args.is_empty()) =>
        {
            cg.line(&format!(
                "let {name} = get_arg(Clean(\"{name}\".to_string())).map(|__v| __v.0);"
            ));
        }
        // Option<Int> → optional integer flag
        TypeExpr::Option { inner, .. } if matches!(unwrap_refined_ty(inner), TypeExpr::Base { name: n, args, .. } if n == "Int" && args.is_empty()) =>
        {
            emit_optional_numeric_parse(cg, name, "i64", "integer");
        }
        // Option<Float> → optional float flag
        TypeExpr::Option { inner, .. } if matches!(unwrap_refined_ty(inner), TypeExpr::Base { name: n, args, .. } if n == "Float" && args.is_empty()) =>
        {
            emit_optional_numeric_parse(cg, name, "f64", "float");
        }
        // String → required string flag
        TypeExpr::Base {
            name: ty_name,
            args,
            ..
        } if ty_name == "String" && args.is_empty() => {
            cg.line(&format!(
                "let {name} = get_arg(Clean(\"{name}\".to_string())).ok_or_else(|| \"missing required argument: --{name}\".to_string())?.0;"
            ));
        }
        // Int → required integer flag
        TypeExpr::Base {
            name: ty_name,
            args,
            ..
        } if ty_name == "Int" && args.is_empty() => {
            emit_required_numeric_parse(cg, name, "i64", "integer");
        }
        // Float → required float flag
        TypeExpr::Base {
            name: ty_name,
            args,
            ..
        } if ty_name == "Float" && args.is_empty() => {
            emit_required_numeric_parse(cg, name, "f64", "float");
        }
        _ => {
            // is_parseable_field_type must be kept in sync with this match.
            unreachable!(
                "emit_field_parse: unhandled parseable type for field `{name}`; \
                 update is_parseable_field_type and add a match arm here"
            );
        }
    }

    // Emit runtime refinement check (returns Err, not debug_assert)
    if let Some(pred) = &field.refinement {
        let pred_str = emit_ref_expr_for_assert(pred, name);
        cg.line(&format!(
            "if !({pred_str}) {{ return Err(format!(\"--{name}: refinement violated: {{}}\", {name})); }}"
        ));
    }
}

/// Emit optional numeric parsing for `Option<Int>` / `Option<Float>` fields.
///
/// `rust_type` is `"i64"` or `"f64"`; `type_label` is `"integer"` or `"float"`.
fn emit_optional_numeric_parse(cg: &mut Codegen, name: &str, rust_type: &str, type_label: &str) {
    cg.line(&format!(
        "let {name} = match get_arg(Clean(\"{name}\".to_string())) {{"
    ));
    cg.push_indent();
    cg.line("None => None,");
    cg.line(&format!(
        "Some(__v) => Some(__v.0.parse::<{rust_type}>().map_err(|_| \"--{name}: expected {type_label}\".to_string())?),"
    ));
    cg.pop_indent();
    cg.line("};");
}

/// Emit required numeric parsing for `Int` / `Float` fields.
///
/// `rust_type` is `"i64"` or `"f64"`; `type_label` is `"integer"` or `"float"`.
fn emit_required_numeric_parse(cg: &mut Codegen, name: &str, rust_type: &str, type_label: &str) {
    cg.line(&format!(
        "let __raw_{name} = get_arg(Clean(\"{name}\".to_string())).ok_or_else(|| \"missing required argument: --{name}\".to_string())?;"
    ));
    cg.line(&format!(
        "let {name} = __raw_{name}.0.parse::<{rust_type}>().map_err(|_| \"--{name}: expected {type_label}\".to_string())?;"
    ));
}

/// Assert that `name` is a safe Rust identifier before interpolating it into
/// generated source code.  Panics at transpile time (not user runtime) if
/// violated, turning a potential codegen-injection into a loud compiler error.
///
/// Valid: `[a-zA-Z_][a-zA-Z0-9_]*`.  The MVL lexer enforces this for all
/// identifiers produced by parsing, but this assertion also guards AST nodes
/// produced by test helpers, fuzzing harnesses, or future parser extensions.
fn assert_safe_identifier(name: &str) {
    assert!(
        !name.is_empty()
            && name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "unsafe identifier in codegen: {name:?}"
    );
}

/// Returns `true` when the MVL type can be parsed from a CLI flag string.
///
/// Parseable: Int, Float, String, Bool, Option<Int>, Option<Float>,
/// Option<String>, and refined wrappers of the above.
fn is_parseable_field_type(ty: &TypeExpr) -> bool {
    match unwrap_refined_ty(ty) {
        TypeExpr::Base { name, args, .. } => {
            args.is_empty() && matches!(name.as_str(), "Int" | "Float" | "String" | "Bool")
        }
        TypeExpr::Option { inner, .. } => {
            matches!(
                unwrap_refined_ty(inner),
                TypeExpr::Base { name, args, .. }
                    if args.is_empty()
                    && matches!(name.as_str(), "Int" | "Float" | "String")
            )
        }
        _ => false,
    }
}

/// Strip a `Refined { inner, .. }` wrapper, returning the inner type.
/// Returns the type unchanged for all other variants.
fn unwrap_refined_ty(ty: &TypeExpr) -> &TypeExpr {
    match ty {
        TypeExpr::Refined { inner, .. } => unwrap_refined_ty(inner),
        other => other,
    }
}

// ── Enum ──────────────────────────────────────────────────────────────────

fn emit_enum(cg: &mut Codegen, name: &str, params: &[GenericParam], variants: &[Variant]) {
    emit_derive(cg, &["Debug", "Clone", "PartialEq"]);
    cg.line(&format!("pub enum {}{} {{", name, generic_params(params)));
    cg.push_indent();
    for v in variants {
        match &v.fields {
            VariantFields::Unit => cg.line(&format!("{},", v.name)),
            VariantFields::Tuple(tys) => {
                let tys_str: Vec<String> = tys.iter().map(emit_type_expr).collect();
                cg.line(&format!("{}({}),", v.name, tys_str.join(", ")));
            }
            VariantFields::Struct(fields) => {
                cg.line(&format!("{} {{", v.name));
                cg.push_indent();
                for f in fields {
                    let ty_str = emit_type_expr(&f.ty);
                    cg.line(&format!("{}: {},", f.name, ty_str));
                }
                cg.pop_indent();
                cg.line("},");
            }
        }
    }
    cg.pop_indent();
    cg.line("}");
}

// ── Type alias / refined alias ────────────────────────────────────────────

fn emit_alias(cg: &mut Codegen, name: &str, params: &[GenericParam], ty: &TypeExpr) {
    match ty {
        TypeExpr::Refined { inner, pred, .. } => {
            // Refined alias becomes a newtype with constructor validation
            let inner_str = emit_type_expr(inner);
            // Add Copy when the inner type is a primitive (i64, f64, bool, char, u8)
            if is_copy_primitive(inner) {
                emit_derive(cg, &["Debug", "Clone", "Copy", "PartialEq", "PartialOrd"]);
            } else {
                emit_derive(cg, &["Debug", "Clone", "PartialEq", "PartialOrd"]);
            }
            cg.line(&format!(
                "pub struct {}{}(pub {});",
                name,
                generic_params(params),
                inner_str
            ));
            cg.blank();
            cg.line(&format!("impl {} {{", name));
            cg.push_indent();
            cg.line(&format!(
                "/// Construct `{name}` — panics in debug mode if the refinement is violated."
            ));
            cg.line(&format!("pub fn new(v: {inner_str}) -> Self {{"));
            cg.push_indent();
            let pred_str = emit_ref_expr_for_assert(pred, "v");
            cg.line(&format!(
                "debug_assert!({pred_str}, \"refinement violated: {name}({{}})\", v);"
            ));
            cg.line("Self(v)");
            cg.pop_indent();
            cg.line("}");
            cg.pop_indent();
            cg.line("}");
        }
        _ => {
            // Plain alias
            let ty_str = emit_type_expr(ty);
            if params.is_empty() {
                cg.line(&format!("pub type {name} = {ty_str};"));
            } else {
                cg.line(&format!(
                    "pub type {}{} = {ty_str};",
                    name,
                    generic_params(params)
                ));
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// Returns true when the MVL type maps to a Rust `Copy` primitive.
fn is_copy_primitive(ty: &TypeExpr) -> bool {
    match ty {
        TypeExpr::Base { name, args, .. } if args.is_empty() => {
            matches!(name.as_str(), "Int" | "Float" | "Bool" | "Char" | "Byte")
        }
        _ => false,
    }
}

fn emit_derive(cg: &mut Codegen, traits: &[&str]) {
    cg.line(&format!("#[derive({})]", traits.join(", ")));
}

fn generic_params(params: &[GenericParam]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = params
        .iter()
        .map(|p| match p {
            GenericParam::Type(name) => name.clone(),
            GenericParam::Const(name, _ty) => format!("const {name}: usize"),
        })
        .collect();
    format!("<{}>", parts.join(", "))
}

// ── TypeExpr → Rust type string ───────────────────────────────────────────

/// Convert an MVL [`TypeExpr`] to its Rust representation.
pub fn emit_type_expr(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::IntConst { value, .. } => {
            if *value < 0 {
                // Negative array sizes are invalid — emit a sentinel that will produce a clear
                // Rust compile error rather than silently emitting a negative literal.
                "__mvl_invalid_negative_array_size__".to_string()
            } else {
                value.to_string()
            }
        }
        TypeExpr::Base { name, args, .. } => {
            // Array<T, N> → [T; N]
            if name == "Array" && args.len() == 2 {
                let elem = emit_type_expr(&args[0]);
                let size = emit_type_expr(&args[1]);
                return format!("[{elem}; {size}]");
            }
            let rust_name = map_base_type(name);
            if args.is_empty() {
                rust_name.to_string()
            } else {
                let args_str: Vec<String> = args.iter().map(emit_type_expr).collect();
                format!("{}<{}>", rust_name, args_str.join(", "))
            }
        }
        TypeExpr::Option { inner, .. } => format!("Option<{}>", emit_type_expr(inner)),
        TypeExpr::Result { ok, err, .. } => {
            format!("Result<{}, {}>", emit_type_expr(ok), emit_type_expr(err))
        }
        TypeExpr::Ref { mutable, inner, .. } => {
            if *mutable {
                format!("&mut {}", emit_type_expr(inner))
            } else {
                format!("&{}", emit_type_expr(inner))
            }
        }
        TypeExpr::Labeled { label, inner, .. } => {
            let label_name = emit_label(*label);
            format!("{}<{}>", label_name, emit_type_expr(inner))
        }
        TypeExpr::Refined { inner, .. } => {
            // Erase refinement at the type level; the newtype constructor handles it
            emit_type_expr(inner)
        }
        TypeExpr::Fn { params, ret, .. } => {
            let params_str: Vec<String> = params.iter().map(emit_type_expr).collect();
            format!("fn({}) -> {}", params_str.join(", "), emit_type_expr(ret))
        }
        TypeExpr::Tuple { elems, .. } => {
            let elems_str: Vec<String> = elems.iter().map(emit_type_expr).collect();
            format!("({})", elems_str.join(", "))
        }
    }
}

/// Map MVL primitive names to Rust equivalents.
fn map_base_type(name: &str) -> &str {
    match name {
        "Int" => "i64",
        "Float" => "f64",
        "Bool" => "bool",
        "String" => "String",
        "Char" => "char",
        "Byte" => "u8",
        "Unit" => "()",
        "Never" => "!",
        "List" => "Vec",
        "Map" => "std::collections::HashMap",
        "Set" => "std::collections::HashSet",
        // Phase 1: unknown types are passed through as-is (user-defined or external)
        other => other,
    }
}

pub fn emit_label(label: SecurityLabel) -> &'static str {
    match label {
        SecurityLabel::Public => "Public",
        SecurityLabel::Tainted => "Tainted",
        SecurityLabel::Secret => "Secret",
        SecurityLabel::Clean => "Clean",
    }
}

// ── Refinement predicate → Rust assert expression ─────────────────────────

/// Emit a refinement predicate as a Rust boolean expression suitable for
/// use inside `debug_assert!(…)`.  The binding name `self` is replaced with
/// `binding`.
pub fn emit_ref_expr_for_assert(pred: &RefExpr, binding: &str) -> String {
    emit_ref_expr(pred, binding)
}

fn emit_ref_expr(pred: &RefExpr, binding: &str) -> String {
    match pred {
        RefExpr::LogicOp {
            op, left, right, ..
        } => {
            let op_str = match op {
                crate::mvl::parser::ast::LogicOp::And => "&&",
                crate::mvl::parser::ast::LogicOp::Or => "||",
            };
            format!(
                "({} {op_str} {})",
                emit_ref_expr(left, binding),
                emit_ref_expr(right, binding)
            )
        }
        RefExpr::Compare {
            op, left, right, ..
        } => {
            let op_str = match op {
                crate::mvl::parser::ast::CmpOp::Eq => "==",
                crate::mvl::parser::ast::CmpOp::Ne => "!=",
                crate::mvl::parser::ast::CmpOp::Lt => "<",
                crate::mvl::parser::ast::CmpOp::Gt => ">",
                crate::mvl::parser::ast::CmpOp::Le => "<=",
                crate::mvl::parser::ast::CmpOp::Ge => ">=",
            };
            format!(
                "({} {op_str} {})",
                emit_ref_expr(left, binding),
                emit_ref_expr(right, binding)
            )
        }
        RefExpr::ArithOp {
            op, left, right, ..
        } => {
            let op_str = match op {
                crate::mvl::parser::ast::ArithOp::Add => "+",
                crate::mvl::parser::ast::ArithOp::Sub => "-",
                crate::mvl::parser::ast::ArithOp::Mul => "*",
                crate::mvl::parser::ast::ArithOp::Div => "/",
                crate::mvl::parser::ast::ArithOp::Rem => "%",
            };
            format!(
                "({} {op_str} {})",
                emit_ref_expr(left, binding),
                emit_ref_expr(right, binding)
            )
        }
        RefExpr::Not { inner, .. } => format!("!{}", emit_ref_expr(inner, binding)),
        RefExpr::Ident { name, .. } => {
            if name == "self" {
                binding.to_string()
            } else {
                name.clone()
            }
        }
        RefExpr::Integer { value, .. } => value.to_string(),
        RefExpr::Float { value, .. } => {
            let s = format!("{value}");
            if s.contains('.') {
                s
            } else {
                format!("{s}.0")
            }
        }
        RefExpr::Len { ident, .. } => {
            if ident == "self" {
                format!("{binding}.len()")
            } else {
                format!("{ident}.len()")
            }
        }
        RefExpr::Grouped { inner, .. } => format!("({})", emit_ref_expr(inner, binding)),
    }
}

// ── Float literal in refinement ───────────────────────────────────────────
// Note: RefExpr::Integer covers integer constants; float literals in
// refinements (e.g. `self >= 0.0`) are not yet in the RefExpr grammar —
// they would need to be added as RefExpr::Float. For now, integer literals
// cover most practical predicates.
