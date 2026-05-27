// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl openapi <file|dir>` — generate an OpenAPI 3.0.3 JSON spec from MVL
//! route tables and handler type signatures.
//!
//! Reads the AST of all `.mvl` files, finds `route()` calls to build the path
//! table, resolves handler function signatures, and maps MVL types (including
//! refinement predicates, effects, and IFC labels) to JSON Schema / OpenAPI
//! constructs.  Output is valid JSON written to stdout.

use mvl::mvl::loader;
use mvl::mvl::parser::ast::{
    ArithOp, CmpOp, Decl, FieldDecl, FnDecl, Program, RefExpr, Stmt, TypeBody, TypeDecl, TypeExpr,
};
use std::collections::HashMap;
use std::process;

// ── Extracted route ──────────────────────────────────────────────────────────

struct ExtractedRoute {
    method: String,
    pattern: String,
    handler_name: String,
}

// ── Entry point ──────────────────────────────────────────────────────────────

pub fn run(path: &str) {
    let files = loader::mvl_files(path, false);
    if files.is_empty() {
        eprintln!("No .mvl files found at: {path}");
        process::exit(1);
    }

    // Parse all files and collect declarations.
    let mut all_programs: Vec<(String, Program)> = Vec::new();
    for f in &files {
        let file_str = f.display().to_string();
        let (prog, _src) = super::parse_or_exit(&file_str);
        all_programs.push((file_str, prog));
    }

    // Collect type declarations and function declarations across all files.
    let mut type_decls: HashMap<String, &TypeDecl> = HashMap::new();
    let mut fn_decls: HashMap<String, &FnDecl> = HashMap::new();
    let mut routes: Vec<ExtractedRoute> = Vec::new();

    for (_file, prog) in &all_programs {
        for decl in &prog.declarations {
            match decl {
                Decl::Type(td) => {
                    type_decls.insert(td.name.clone(), td);
                }
                Decl::Fn(fd) => {
                    fn_decls.insert(fd.name.clone(), fd);
                    // Extract routes from function bodies.
                    extract_routes_from_fn(fd, &mut routes);
                }
                _ => {}
            }
        }
    }

    if routes.is_empty() {
        eprintln!("warning: no routes found (no calls to `route()` detected)");
    }

    // Build and print OpenAPI spec.
    let json = build_openapi_json(&routes, &fn_decls, &type_decls);
    println!("{json}");
}

// ── Route extraction ─────────────────────────────────────────────────────────

/// Walk a function body looking for `route(router, Method::X, "/path", "name")` calls.
fn extract_routes_from_fn(fd: &FnDecl, routes: &mut Vec<ExtractedRoute>) {
    for stmt in &fd.body.stmts {
        extract_routes_from_stmt(stmt, routes);
    }
}

fn extract_routes_from_stmt(stmt: &Stmt, routes: &mut Vec<ExtractedRoute>) {
    match stmt {
        Stmt::Let { init, .. } => extract_routes_from_expr(init, routes),
        Stmt::Expr { expr, .. } => extract_routes_from_expr(expr, routes),
        Stmt::Return {
            value: Some(expr), ..
        } => {
            extract_routes_from_expr(expr, routes);
        }
        _ => {}
    }
}

fn extract_routes_from_expr(expr: &mvl::mvl::parser::ast::Expr, routes: &mut Vec<ExtractedRoute>) {
    use mvl::mvl::parser::ast::Expr;

    match expr {
        Expr::FnCall { name, args, .. } if name == "route" && args.len() == 4 => {
            // route(router, Method::Get, "/path", "handler_name")
            let method = extract_method_from_expr(&args[1]);
            let pattern = extract_string_literal(&args[2]);
            let handler_name = extract_string_literal(&args[3]);

            if let (Some(method), Some(pattern), Some(handler_name)) =
                (method, pattern, handler_name)
            {
                routes.push(ExtractedRoute {
                    method,
                    pattern,
                    handler_name,
                });
            }
        }
        // Recurse into nested expressions that might contain route() calls.
        Expr::FnCall { args, .. } => {
            for arg in args {
                extract_routes_from_expr(arg, routes);
            }
        }
        Expr::Block(block) => {
            for stmt in &block.stmts {
                extract_routes_from_stmt(stmt, routes);
            }
        }
        Expr::If { then, else_, .. } => {
            for stmt in &then.stmts {
                extract_routes_from_stmt(stmt, routes);
            }
            if let Some(e) = else_ {
                extract_routes_from_expr(e, routes);
            }
        }
        _ => {}
    }
}

/// Extract HTTP method from `Method::Get` etc.
fn extract_method_from_expr(expr: &mvl::mvl::parser::ast::Expr) -> Option<String> {
    use mvl::mvl::parser::ast::Expr;
    match expr {
        Expr::FnCall { name, args, .. } if args.is_empty() => {
            name.strip_prefix("Method::").map(|m| m.to_lowercase())
        }
        Expr::Ident(name, _) => name.strip_prefix("Method::").map(|m| m.to_lowercase()),
        _ => None,
    }
}

fn extract_string_literal(expr: &mvl::mvl::parser::ast::Expr) -> Option<String> {
    use mvl::mvl::parser::ast::{Expr, Literal};
    match expr {
        Expr::Literal(Literal::Str(s), _) => Some(s.clone()),
        _ => None,
    }
}

// ── OpenAPI JSON generation ──────────────────────────────────────────────────

fn build_openapi_json(
    routes: &[ExtractedRoute],
    fn_decls: &HashMap<String, &FnDecl>,
    type_decls: &HashMap<String, &TypeDecl>,
) -> String {
    let esc = super::json_escape;
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"openapi\": \"3.0.3\",\n");
    out.push_str("  \"info\": {\n");
    out.push_str("    \"title\": \"MVL Generated API\",\n");
    out.push_str("    \"version\": \"1.0.0\"\n");
    out.push_str("  },\n");

    // Group routes by path.
    let mut paths: Vec<(&str, Vec<&ExtractedRoute>)> = Vec::new();
    for route in routes {
        if let Some(entry) = paths.iter_mut().find(|(p, _)| *p == route.pattern.as_str()) {
            entry.1.push(route);
        } else {
            paths.push((route.pattern.as_str(), vec![route]));
        }
    }

    out.push_str("  \"paths\": {\n");
    for (pi, (path, methods)) in paths.iter().enumerate() {
        // Convert {param} to OpenAPI {param} format (already correct).
        let openapi_path = esc(path);
        out.push_str(&format!("    \"{openapi_path}\": {{\n"));

        for (mi, route) in methods.iter().enumerate() {
            out.push_str(&format!("      \"{}\": {{\n", esc(&route.method)));

            // Operation ID.
            out.push_str(&format!(
                "        \"operationId\": \"{}\",\n",
                esc(&route.handler_name)
            ));

            // Look up the handler function to extract tags, parameters, request body, responses.
            let handler_fn = find_handler_fn(&route.handler_name, fn_decls);

            // Tags from effects.
            if let Some(fd) = handler_fn {
                if !fd.effects.is_empty() {
                    out.push_str("        \"tags\": [");
                    for (ei, eff) in fd.effects.iter().enumerate() {
                        out.push_str(&format!("\"{}\"", esc(&eff.name)));
                        if ei + 1 < fd.effects.len() {
                            out.push_str(", ");
                        }
                    }
                    out.push_str("],\n");
                }
            }

            // Path parameters.
            let path_params = extract_path_params(path);
            if !path_params.is_empty() {
                out.push_str("        \"parameters\": [\n");
                for (ppi, param_name) in path_params.iter().enumerate() {
                    out.push_str("          {\n");
                    out.push_str(&format!("            \"name\": \"{}\",\n", esc(param_name)));
                    out.push_str("            \"in\": \"path\",\n");
                    out.push_str("            \"required\": true,\n");
                    out.push_str("            \"schema\": { \"type\": \"string\" }\n");
                    out.push_str("          }");
                    if ppi + 1 < path_params.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str("        ],\n");
            }

            // Request body — look for a body/Json parameter.
            if let Some(fd) = handler_fn {
                if let Some(body_schema) = extract_request_body_schema(fd, type_decls) {
                    out.push_str("        \"requestBody\": {\n");
                    out.push_str("          \"content\": {\n");
                    out.push_str("            \"application/json\": {\n");
                    out.push_str("              \"schema\": ");
                    out.push_str(&indent_json(&body_schema, 14));
                    out.push('\n');
                    out.push_str("            }\n");
                    out.push_str("          }\n");
                    out.push_str("        },\n");
                }
            }

            // Responses.
            out.push_str("        \"responses\": {\n");
            if let Some(fd) = handler_fn {
                let (success_schema, has_error) = extract_response_schemas(fd, type_decls);
                // Success response.
                out.push_str("          \"200\": {\n");
                out.push_str("            \"description\": \"Successful response\"");
                if let Some(schema) = &success_schema {
                    out.push_str(",\n");
                    out.push_str("            \"content\": {\n");
                    out.push_str("              \"application/json\": {\n");
                    out.push_str("                \"schema\": ");
                    out.push_str(&indent_json(schema, 16));
                    out.push('\n');
                    out.push_str("              }\n");
                    out.push_str("            }\n");
                } else {
                    out.push('\n');
                }
                out.push_str("          }");
                if has_error {
                    out.push_str(",\n");
                    out.push_str("          \"default\": {\n");
                    out.push_str("            \"description\": \"Error response\"\n");
                    out.push_str("          }");
                }
                out.push('\n');
            } else {
                out.push_str("          \"200\": {\n");
                out.push_str("            \"description\": \"Successful response\"\n");
                out.push_str("          }\n");
            }
            out.push_str("        }\n");

            // Close operation.
            out.push_str("      }");
            if mi + 1 < methods.len() {
                out.push(',');
            }
            out.push('\n');
        }

        out.push_str("    }");
        if pi + 1 < paths.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("  }\n");
    out.push_str("}\n");
    // Remove trailing newline for clean output.
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Find handler function by name — try exact match, then `<name>_handler` suffix.
fn find_handler_fn<'a>(
    handler_name: &str,
    fn_decls: &'a HashMap<String, &FnDecl>,
) -> Option<&'a FnDecl> {
    // Route table uses short names like "list_users", handlers may be "list_users_handler".
    fn_decls.get(handler_name).copied().or_else(|| {
        let with_suffix = format!("{handler_name}_handler");
        fn_decls.get(with_suffix.as_str()).copied()
    })
}

/// Extract `{param}` names from a path pattern.
fn extract_path_params(path: &str) -> Vec<String> {
    let mut params = Vec::new();
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut name = String::new();
            for c2 in chars.by_ref() {
                if c2 == '}' {
                    break;
                }
                name.push(c2);
            }
            if !name.is_empty() {
                params.push(name);
            }
        }
    }
    params
}

// ── Request body extraction ──────────────────────────────────────────────────

/// Look at handler parameters for body types (Request with JSON parsing, or
/// direct struct params). Returns a JSON schema string if a body type is found.
fn extract_request_body_schema(
    fd: &FnDecl,
    type_decls: &HashMap<String, &TypeDecl>,
) -> Option<String> {
    for param in &fd.params {
        match &param.ty {
            // Json[CreateUserRequest] → schema for CreateUserRequest
            TypeExpr::Base { name, args, .. } if name == "Json" && args.len() == 1 => {
                return Some(type_expr_to_schema(&args[0], type_decls));
            }
            _ => {}
        }
    }
    // For handlers that use pkg.http style: find parse_*_body helper patterns.
    // In the crud_api example, handlers take (db, req, matched) and internally
    // parse the body. We can't easily trace through call chains, so for non-Json
    // params we look for a matching _body parser naming convention.
    None
}

// ── Response schema extraction ───────────────────────────────────────────────

/// Extract success and error response schemas from the return type.
/// Returns (success_schema, has_error_type).
fn extract_response_schemas(
    fd: &FnDecl,
    type_decls: &HashMap<String, &TypeDecl>,
) -> (Option<String>, bool) {
    match fd.return_type.as_ref() {
        // Result[Json[User], HttpError] → success=User schema, has_error=true
        TypeExpr::Result { ok, .. } => {
            let schema = unwrap_json_type(ok, type_decls);
            (schema, true)
        }
        // Json[User] → success=User schema
        TypeExpr::Base { name, args, .. } if name == "Json" && args.len() == 1 => {
            (Some(type_expr_to_schema(&args[0], type_decls)), false)
        }
        // Response (opaque) — no schema
        TypeExpr::Base { name, .. } if name == "Response" => (None, false),
        other => {
            let schema = type_expr_to_schema(other, type_decls);
            if schema == "{}" {
                (None, false)
            } else {
                (Some(schema), false)
            }
        }
    }
}

/// Unwrap Json[T] wrapper if present, returning a schema for T.
fn unwrap_json_type(ty: &TypeExpr, type_decls: &HashMap<String, &TypeDecl>) -> Option<String> {
    match ty {
        TypeExpr::Base { name, args, .. } if name == "Json" && args.len() == 1 => {
            Some(type_expr_to_schema(&args[0], type_decls))
        }
        _ => Some(type_expr_to_schema(ty, type_decls)),
    }
}

// ── Type → JSON Schema ──────────────────────────────────────────────────────

fn type_expr_to_schema(ty: &TypeExpr, type_decls: &HashMap<String, &TypeDecl>) -> String {
    let esc = super::json_escape;
    match ty {
        TypeExpr::Base { name, args, .. } => match name.as_str() {
            "Int" => "{ \"type\": \"integer\" }".to_string(),
            "Float" => "{ \"type\": \"number\" }".to_string(),
            "String" => "{ \"type\": \"string\" }".to_string(),
            "Bool" => "{ \"type\": \"boolean\" }".to_string(),
            "Unit" => "{}".to_string(),
            "List" if args.len() == 1 => {
                let items = type_expr_to_schema(&args[0], type_decls);
                format!("{{ \"type\": \"array\", \"items\": {items} }}")
            }
            "Map" if args.len() == 2 => {
                let values = type_expr_to_schema(&args[1], type_decls);
                format!("{{ \"type\": \"object\", \"additionalProperties\": {values} }}")
            }
            "Json" if args.len() == 1 => type_expr_to_schema(&args[0], type_decls),
            _ => {
                // Look up user-defined type.
                if let Some(td) = type_decls.get(name.as_str()) {
                    type_decl_to_schema(td, type_decls)
                } else {
                    // Unknown type → open schema (acceptance criterion).
                    "{}".to_string()
                }
            }
        },
        TypeExpr::Option { inner, .. } => {
            let inner_schema = type_expr_to_schema(inner, type_decls);
            format!("{{ \"nullable\": true, \"allOf\": [{inner_schema}] }}")
        }
        TypeExpr::Result { ok, .. } => {
            // Unwrap Result to the success type.
            type_expr_to_schema(ok, type_decls)
        }
        TypeExpr::Refined { inner, pred, .. } => {
            let base = type_expr_to_schema(inner, type_decls);
            apply_refinement_to_schema(&base, inner, pred)
        }
        TypeExpr::Labeled { label, inner, .. } => {
            // IFC labels → x-security-label annotation.
            let inner_schema = type_expr_to_schema(inner, type_decls);
            inject_property(
                &inner_schema,
                &format!("\"x-security-label\": \"{}\"", esc(label)),
            )
        }
        _ => "{}".to_string(),
    }
}

fn type_decl_to_schema(td: &TypeDecl, type_decls: &HashMap<String, &TypeDecl>) -> String {
    let esc = super::json_escape;
    match &td.body {
        TypeBody::Struct { fields, .. } => struct_fields_to_schema(&td.name, fields, type_decls),
        TypeBody::Enum(variants) => {
            // Simple enum → string enum.
            let mut out = String::from("{ \"type\": \"string\", \"enum\": [");
            for (i, v) in variants.iter().enumerate() {
                out.push_str(&format!("\"{}\"", esc(&v.name)));
                if i + 1 < variants.len() {
                    out.push_str(", ");
                }
            }
            out.push_str("] }");
            out
        }
        TypeBody::Alias(inner) => type_expr_to_schema(inner, type_decls),
    }
}

fn struct_fields_to_schema(
    _name: &str,
    fields: &[FieldDecl],
    type_decls: &HashMap<String, &TypeDecl>,
) -> String {
    let esc = super::json_escape;
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("              \"type\": \"object\",\n");

    // Properties.
    out.push_str("              \"properties\": {\n");
    for (i, field) in fields.iter().enumerate() {
        let field_schema = field_to_schema(field, type_decls);
        out.push_str(&format!(
            "                \"{}\": {}",
            esc(&field.name),
            field_schema
        ));
        if i + 1 < fields.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("              },\n");

    // All struct fields are required (MVL has no optional fields in structs — use Option[T]).
    out.push_str("              \"required\": [");
    for (i, field) in fields.iter().enumerate() {
        out.push_str(&format!("\"{}\"", esc(&field.name)));
        if i + 1 < fields.len() {
            out.push_str(", ");
        }
    }
    out.push_str("]\n");
    out.push_str("            }");
    out
}

fn field_to_schema(field: &FieldDecl, type_decls: &HashMap<String, &TypeDecl>) -> String {
    let base_ty = &field.ty;
    let base_schema = type_expr_to_schema(base_ty, type_decls);

    // Apply field-level refinement if present.
    if let Some(ref pred) = field.refinement {
        apply_refinement_to_schema(&base_schema, base_ty, pred)
    } else {
        base_schema
    }
}

// ── Refinement → OpenAPI validation keywords ─────────────────────────────────

/// Apply a refinement predicate to an existing JSON schema string.
/// Only handles simple Layer-1 patterns: comparisons on `self` and `len(self)`.
fn apply_refinement_to_schema(base_schema: &str, base_type: &TypeExpr, pred: &RefExpr) -> String {
    let mut constraints: Vec<String> = Vec::new();
    collect_refinement_constraints(pred, base_type, &mut constraints);

    if constraints.is_empty() {
        return base_schema.to_string();
    }

    // Inject constraints into the base schema.
    let extra = constraints.join(", ");
    inject_property(base_schema, &extra)
}

/// Recursively collect OpenAPI constraint strings from a refinement predicate.
fn collect_refinement_constraints(
    pred: &RefExpr,
    base_type: &TypeExpr,
    constraints: &mut Vec<String>,
) {
    match pred {
        // self > N or self >= N (on Int → minimum)
        RefExpr::Compare {
            op, left, right, ..
        } => {
            if is_self_ident(left) {
                if let Some(n) = extract_integer(right) {
                    match (op, is_string_type(base_type)) {
                        (CmpOp::Gt, false) => {
                            constraints.push(format!("\"minimum\": {}", n + 1));
                        }
                        (CmpOp::Ge, false) => {
                            constraints.push(format!("\"minimum\": {n}"));
                        }
                        (CmpOp::Lt, false) => {
                            constraints.push(format!("\"maximum\": {}", n - 1));
                        }
                        (CmpOp::Le, false) => {
                            constraints.push(format!("\"maximum\": {n}"));
                        }
                        _ => {}
                    }
                }
            }
            // len(self) > N or len(self) >= N (on String → minLength)
            if is_len_self(left) {
                if let Some(n) = extract_integer(right) {
                    match op {
                        CmpOp::Gt => {
                            constraints.push(format!("\"minLength\": {}", n + 1));
                        }
                        CmpOp::Ge => {
                            constraints.push(format!("\"minLength\": {n}"));
                        }
                        CmpOp::Lt => {
                            constraints.push(format!("\"maxLength\": {}", n - 1));
                        }
                        CmpOp::Le => {
                            constraints.push(format!("\"maxLength\": {n}"));
                        }
                        _ => {}
                    }
                }
            }
            // N < self → self > N (flipped)
            if is_self_ident(right) {
                if let Some(n) = extract_integer(left) {
                    match (op.flip(), is_string_type(base_type)) {
                        (CmpOp::Gt, false) => {
                            constraints.push(format!("\"minimum\": {}", n + 1));
                        }
                        (CmpOp::Ge, false) => {
                            constraints.push(format!("\"minimum\": {n}"));
                        }
                        (CmpOp::Lt, false) => {
                            constraints.push(format!("\"maximum\": {}", n - 1));
                        }
                        (CmpOp::Le, false) => {
                            constraints.push(format!("\"maximum\": {n}"));
                        }
                        _ => {}
                    }
                }
            }
        }
        // &&: recurse both sides.
        RefExpr::LogicOp {
            op: mvl::mvl::parser::ast::LogicOp::And,
            left,
            right,
            ..
        } => {
            collect_refinement_constraints(left, base_type, constraints);
            collect_refinement_constraints(right, base_type, constraints);
        }
        RefExpr::Grouped { inner, .. } => {
            collect_refinement_constraints(inner, base_type, constraints);
        }
        _ => {}
    }
}

fn is_self_ident(expr: &RefExpr) -> bool {
    matches!(expr, RefExpr::Ident { name, .. } if name == "self")
}

fn is_len_self(expr: &RefExpr) -> bool {
    matches!(expr, RefExpr::Len { ident, .. } if ident == "self")
}

fn extract_integer(expr: &RefExpr) -> Option<i64> {
    match expr {
        RefExpr::Integer { value, .. } => Some(*value),
        // Handle 0 - N for negative literals.
        RefExpr::ArithOp {
            op: ArithOp::Sub,
            left,
            right,
            ..
        } => {
            if let (RefExpr::Integer { value: 0, .. }, RefExpr::Integer { value: n, .. }) =
                (left.as_ref(), right.as_ref())
            {
                Some(-n)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn is_string_type(ty: &TypeExpr) -> bool {
    matches!(ty, TypeExpr::Base { name, .. } if name == "String")
}

// ── JSON helpers ─────────────────────────────────────────────────────────────

/// Inject additional properties into a `{ ... }` JSON object string.
fn inject_property(json: &str, extra: &str) -> String {
    let trimmed = json.trim();
    if trimmed == "{}" {
        return format!("{{ {extra} }}");
    }
    // Find the last `}` and insert before it.
    if let Some(pos) = trimmed.rfind('}') {
        let before = trimmed[..pos].trim_end();
        // Add comma if needed.
        let sep = if before.ends_with('{') || before.ends_with(',') {
            " "
        } else {
            ", "
        };
        format!("{before}{sep}{extra} }}")
    } else {
        format!("{{ {extra} }}")
    }
}

/// Indent a JSON string to align with surrounding output.
fn indent_json(json: &str, _indent: usize) -> String {
    // For now, just return inline (single-line schemas are fine).
    json.to_string()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mvl::mvl::parser::ast::{CmpOp, LogicOp, RefExpr, TypeExpr};
    use mvl::mvl::parser::lexer::Span;

    fn dummy() -> Span {
        Span::default()
    }

    #[test]
    fn extract_path_params_basic() {
        assert_eq!(extract_path_params("/users"), Vec::<String>::new());
        assert_eq!(extract_path_params("/users/{id}"), vec!["id"]);
        assert_eq!(
            extract_path_params("/users/{id}/posts/{post_id}"),
            vec!["id", "post_id"]
        );
    }

    #[test]
    fn int_type_to_schema() {
        let td: HashMap<String, &TypeDecl> = HashMap::new();
        let ty = TypeExpr::Base {
            name: "Int".into(),
            args: vec![],
            span: dummy(),
        };
        assert_eq!(type_expr_to_schema(&ty, &td), "{ \"type\": \"integer\" }");
    }

    #[test]
    fn string_type_to_schema() {
        let td: HashMap<String, &TypeDecl> = HashMap::new();
        let ty = TypeExpr::Base {
            name: "String".into(),
            args: vec![],
            span: dummy(),
        };
        assert_eq!(type_expr_to_schema(&ty, &td), "{ \"type\": \"string\" }");
    }

    #[test]
    fn unknown_type_to_open_schema() {
        let td: HashMap<String, &TypeDecl> = HashMap::new();
        let ty = TypeExpr::Base {
            name: "Foo".into(),
            args: vec![],
            span: dummy(),
        };
        assert_eq!(type_expr_to_schema(&ty, &td), "{}");
    }

    #[test]
    fn list_type_to_schema() {
        let td: HashMap<String, &TypeDecl> = HashMap::new();
        let ty = TypeExpr::Base {
            name: "List".into(),
            args: vec![TypeExpr::Base {
                name: "Int".into(),
                args: vec![],
                span: dummy(),
            }],
            span: dummy(),
        };
        assert_eq!(
            type_expr_to_schema(&ty, &td),
            "{ \"type\": \"array\", \"items\": { \"type\": \"integer\" } }"
        );
    }

    #[test]
    fn refinement_self_gt_zero_int() {
        let td: HashMap<String, &TypeDecl> = HashMap::new();
        let ty = TypeExpr::Refined {
            inner: Box::new(TypeExpr::Base {
                name: "Int".into(),
                args: vec![],
                span: dummy(),
            }),
            pred: RefExpr::Compare {
                op: CmpOp::Gt,
                left: Box::new(RefExpr::Ident {
                    name: "self".into(),
                    span: dummy(),
                }),
                right: Box::new(RefExpr::Integer {
                    value: 0,
                    span: dummy(),
                }),
                span: dummy(),
            },
            span: dummy(),
        };
        let schema = type_expr_to_schema(&ty, &td);
        assert!(schema.contains("\"minimum\": 1"), "got: {schema}");
        assert!(schema.contains("\"type\": \"integer\""), "got: {schema}");
    }

    #[test]
    fn refinement_self_ge_18_int() {
        let td: HashMap<String, &TypeDecl> = HashMap::new();
        let ty = TypeExpr::Refined {
            inner: Box::new(TypeExpr::Base {
                name: "Int".into(),
                args: vec![],
                span: dummy(),
            }),
            pred: RefExpr::Compare {
                op: CmpOp::Ge,
                left: Box::new(RefExpr::Ident {
                    name: "self".into(),
                    span: dummy(),
                }),
                right: Box::new(RefExpr::Integer {
                    value: 18,
                    span: dummy(),
                }),
                span: dummy(),
            },
            span: dummy(),
        };
        let schema = type_expr_to_schema(&ty, &td);
        assert!(schema.contains("\"minimum\": 18"), "got: {schema}");
    }

    #[test]
    fn refinement_len_self_gt_zero_string() {
        let td: HashMap<String, &TypeDecl> = HashMap::new();
        let ty = TypeExpr::Refined {
            inner: Box::new(TypeExpr::Base {
                name: "String".into(),
                args: vec![],
                span: dummy(),
            }),
            pred: RefExpr::Compare {
                op: CmpOp::Gt,
                left: Box::new(RefExpr::Len {
                    ident: "self".into(),
                    span: dummy(),
                }),
                right: Box::new(RefExpr::Integer {
                    value: 0,
                    span: dummy(),
                }),
                span: dummy(),
            },
            span: dummy(),
        };
        let schema = type_expr_to_schema(&ty, &td);
        assert!(schema.contains("\"minLength\": 1"), "got: {schema}");
    }

    #[test]
    fn compound_refinement_and() {
        let td: HashMap<String, &TypeDecl> = HashMap::new();
        // Int where self >= 0 && self <= 100
        let ty = TypeExpr::Refined {
            inner: Box::new(TypeExpr::Base {
                name: "Int".into(),
                args: vec![],
                span: dummy(),
            }),
            pred: RefExpr::LogicOp {
                op: LogicOp::And,
                left: Box::new(RefExpr::Compare {
                    op: CmpOp::Ge,
                    left: Box::new(RefExpr::Ident {
                        name: "self".into(),
                        span: dummy(),
                    }),
                    right: Box::new(RefExpr::Integer {
                        value: 0,
                        span: dummy(),
                    }),
                    span: dummy(),
                }),
                right: Box::new(RefExpr::Compare {
                    op: CmpOp::Le,
                    left: Box::new(RefExpr::Ident {
                        name: "self".into(),
                        span: dummy(),
                    }),
                    right: Box::new(RefExpr::Integer {
                        value: 100,
                        span: dummy(),
                    }),
                    span: dummy(),
                }),
                span: dummy(),
            },
            span: dummy(),
        };
        let schema = type_expr_to_schema(&ty, &td);
        assert!(schema.contains("\"minimum\": 0"), "got: {schema}");
        assert!(schema.contains("\"maximum\": 100"), "got: {schema}");
    }

    #[test]
    fn ifc_label_adds_security_annotation() {
        let td: HashMap<String, &TypeDecl> = HashMap::new();
        let ty = TypeExpr::Labeled {
            label: "Tainted".to_string(),
            inner: Box::new(TypeExpr::Base {
                name: "String".into(),
                args: vec![],
                span: dummy(),
            }),
            span: dummy(),
        };
        let schema = type_expr_to_schema(&ty, &td);
        assert!(
            schema.contains("\"x-security-label\": \"Tainted\""),
            "got: {schema}"
        );
    }

    #[test]
    fn inject_property_into_empty() {
        assert_eq!(inject_property("{}", "\"foo\": 1"), "{ \"foo\": 1 }");
    }

    #[test]
    fn inject_property_into_existing() {
        let result = inject_property("{ \"type\": \"integer\" }", "\"minimum\": 1");
        assert!(result.contains("\"type\": \"integer\""), "got: {result}");
        assert!(result.contains("\"minimum\": 1"), "got: {result}");
    }
}
