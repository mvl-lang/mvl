// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Declaration registration and checking for the MVL type checker.
//!
//! Pass 1: `collect_declarations` populates the type/function tables.
//! Pass 2: `check_fn_decl`, `check_extern_decl`, `check_const_decl` verify bodies.

use crate::mvl::checker::context::{
    field_infos, variant_infos, FnInfo, TypeBodyInfo, TypeInfo, VarInfo,
};
use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::types::{resolve, types_compatible, Ty};
use crate::mvl::parser::ast::{ConstDecl, Decl, ExternDecl, FnDecl, ImplDecl, TypeBody, TypeDecl};
use crate::mvl::parser::lexer::Span;
use std::collections::{HashMap, HashSet};

use super::capabilities::block_return_flows_from_ref_param;
use super::{TypeChecker, VALID_EFFECT_NAMES};

impl TypeChecker {
    pub(super) fn collect_declarations(&mut self, decls: &[Decl]) {
        for decl in decls {
            match decl {
                Decl::Type(td) => self.register_type(td),
                Decl::Fn(fd) => self.register_fn(fd),
                Decl::Const(_) => {}
                Decl::Extern(ed) => self.register_extern(ed),
                Decl::Use(_) => {} // resolved by the module resolver, not the type checker
                Decl::Impl(id) => self.register_impl(id),
            }
        }
    }

    fn register_type(&mut self, td: &TypeDecl) {
        let body_info = match &td.body {
            TypeBody::Struct(fields) => TypeBodyInfo::Struct(field_infos(fields)),
            TypeBody::Enum(variants) => TypeBodyInfo::Enum(variant_infos(variants)),
            TypeBody::Alias(ty_expr) => TypeBodyInfo::Alias(resolve(ty_expr)),
        };
        self.env.define_type(
            td.name.clone(),
            TypeInfo {
                params: td.params.clone(),
                body: body_info,
            },
        );
    }

    fn register_fn(&mut self, fd: &FnDecl) {
        let params: Vec<Ty> = fd.params.iter().map(|p| resolve(&p.ty)).collect();
        let ret = resolve(&fd.return_type);
        let type_params = fd
            .type_params
            .iter()
            .map(|p| p.name().to_string())
            .collect();
        self.env.define_fn(
            fd.name.clone(),
            FnInfo {
                params,
                ret,
                effects: fd.effects.clone(),
                totality: fd.totality.clone(),
                type_params,
                label_transparent: fd.is_label_transparent,
            },
        );
    }

    /// Register all functions declared inside an `extern` block so that MVL
    /// callers can resolve them as regular function calls.
    fn register_extern(&mut self, ed: &ExternDecl) {
        // Note: extern_count is incremented in check_extern_decl (pass 2) after
        // ABI validation, not here, so rejected blocks don't inflate the count.
        for f in &ed.fns {
            let params: Vec<Ty> = f.params.iter().map(|p| resolve(&p.ty)).collect();
            let ret = resolve(&f.return_type);
            self.env.define_fn(
                f.name.clone(),
                FnInfo {
                    params,
                    ret,
                    effects: f.effects.clone(),
                    totality: None,
                    type_params: HashSet::new(), // extern fns may or may not terminate
                    label_transparent: false,
                },
            );
        }
    }

    /// Register trait implementations for use during type checking.
    /// - `impl From<A> for B` → enables `?` propagation
    /// - `impl Iterator<T> for X` → enables `X` in `for...in` loops
    fn register_impl(&mut self, id: &ImplDecl) {
        if id.trait_name == "From" {
            if let Some(source_ty) = id.trait_type_args.first() {
                let source = resolve(source_ty).display();
                self.env.register_from_impl(id.type_name.clone(), source);
            }
        } else if id.trait_name == "Iterator" {
            let elem_ty = id
                .trait_type_args
                .first()
                .map(resolve)
                .unwrap_or(Ty::Unknown);
            self.iterator_impls.insert(id.type_name.clone(), elem_ty);
        }
    }

    /// Return the iterator element type for `ty`, or emit `NotIterator` and return `Unknown`.
    ///
    /// Accepted iterator types (001-type-system Req 11):
    /// - `List<T>` — treated as `Iterator<T>` (existing behavior)
    /// - `Array<T, N>` — built-in `Iterator<T>` implementation
    /// - Any named type registered via `impl Iterator<T> for X`
    pub(super) fn check_iterator_type(&mut self, ty: &Ty, span: Span) -> Ty {
        let unlabeled = ty.unlabeled();
        // Built-in iterable types.
        match unlabeled {
            Ty::List(inner) | Ty::Array(inner, _) => return *inner.clone(),
            Ty::Unknown => return Ty::Unknown, // propagate without double-reporting
            _ => {}
        }
        // User-declared iterator implementations.
        if let Ty::Named(name, _) = unlabeled {
            if let Some(elem) = self.iterator_impls.get(name).cloned() {
                return elem;
            }
        }
        self.emit(CheckError::NotIterator {
            ty: ty.display(),
            span,
        });
        Ty::Unknown
    }

    // ── Declarations ─────────────────────────────────────────────────────

    pub(super) fn check_decl(&mut self, decl: &Decl) {
        match decl {
            Decl::Type(_) => {} // type declarations are structurally valid if parsed
            Decl::Fn(fd) => self.check_fn_decl(fd),
            Decl::Const(cd) => self.check_const_decl(cd),
            Decl::Extern(ed) => self.check_extern_decl(ed),
            Decl::Use(_) => {} // resolved by the module resolver, not the type checker
            Decl::Impl(_) => {} // bodies not yet type-checked; registration done in collect_declarations
        }
    }

    fn check_extern_decl(&mut self, ed: &ExternDecl) {
        // Validate ABI string: only "rust" and "c" are supported.
        // Unsupported ABIs are rejected and do NOT count toward the assurance surface.
        if ed.abi != "rust" && ed.abi != "c" {
            self.emit(CheckError::UnsupportedExternAbi {
                abi: ed.abi.clone(),
                span: ed.span,
            });
            return;
        }
        // Count only validated extern blocks in the assurance metric.
        self.extern_count += 1;
        // Each extern fn must have a valid return type (basic check).
        // Future: verify no MVL-specific types (security labels) cross the boundary
        // without explicit wrapping — for now we accept all types.
    }

    fn check_fn_decl(&mut self, fd: &FnDecl) {
        // Builtin functions have no body — skip body checking entirely.
        // Their signatures are registered (in collect_declarations) and trusted.
        if fd.is_builtin {
            return;
        }

        // ADR-0024: validate transparent function constraints.
        if fd.is_label_transparent {
            if fd.params.is_empty() {
                self.emit(CheckError::TransparentFnNoParams {
                    name: fd.name.clone(),
                    span: fd.span,
                });
            }
            if !fd.type_params.is_empty() {
                self.emit(CheckError::TransparentFnGeneric {
                    name: fd.name.clone(),
                    span: fd.span,
                });
            }
            let ret_ty = resolve(&fd.return_type);
            if matches!(ret_ty, crate::mvl::checker::types::Ty::Labeled(..)) {
                self.emit(CheckError::TransparentFnLabeledReturn {
                    name: fd.name.clone(),
                    span: fd.span,
                });
            }
        }

        let ret_ty = resolve(&fd.return_type);
        let prev_ret = self.current_return_ty.replace(ret_ty.clone());

        // Phase C (Spec 009 Req 2): scope-based lifetime check.
        // If the return type is val T / ref T and the function has no val/ref parameters,
        // the reference can only point to a local variable — which would be deallocated
        // when the function returns.  Reject this statically.
        // Additionally verify that the tail expression actually flows from one of those
        // val/ref parameters (not from a local variable or literal).
        if matches!(ret_ty, Ty::Ref(_, _)) {
            let ref_param_names: HashSet<&str> = fd
                .params
                .iter()
                .filter(|p| matches!(resolve(&p.ty), Ty::Ref(_, _)))
                .map(|p| p.name.as_str())
                .collect();
            if ref_param_names.is_empty() {
                self.emit(CheckError::ReferenceEscapesScope {
                    name: fd.name.clone(),
                    span: fd.span,
                });
            } else if let Some(bad_span) =
                block_return_flows_from_ref_param(&fd.body, &ref_param_names)
            {
                self.emit(CheckError::ReferenceEscapesScope {
                    name: fd.name.clone(),
                    span: bad_span,
                });
            }
        }

        // Validate effect names against the canonical set (002-effect-system/Req 2).
        for effect in &fd.effects {
            if !VALID_EFFECT_NAMES.contains(&effect.name.as_str()) {
                self.emit(CheckError::InvalidEffectName {
                    name: effect.name.clone(),
                    span: effect.span,
                });
            }
        }

        // Save and set effect/totality context (Req 7, 8, 9).
        let prev_fn_name = std::mem::replace(&mut self.current_fn_name, fd.name.clone());
        let prev_effects = std::mem::replace(&mut self.current_fn_effects, fd.effects.clone());
        let prev_totality = std::mem::replace(&mut self.current_fn_totality, fd.totality.clone());

        // Build type-param constraint context (001-type-system/Req 9).
        let type_params: HashSet<String> = fd
            .type_params
            .iter()
            .map(|p| p.name().to_string())
            .collect();
        let mut type_constraints: HashMap<String, Vec<String>> = HashMap::new();
        for c in &fd.constraints {
            type_constraints
                .entry(c.name.clone())
                .or_default()
                .push(c.bound.clone());
        }
        let prev_type_params = std::mem::replace(&mut self.current_type_params, type_params);
        let prev_type_constraints =
            std::mem::replace(&mut self.current_type_constraints, type_constraints);

        // Phase D (Spec 009 Req 2): mutable-reference alias check.
        // Two `ref T` parameters of the same inner type, or a `val T` + `ref T` pair,
        // could be aliased at a call site.  Reject both statically.
        // Two-pass: collect all `val T` inner types first so the check is order-independent.
        {
            let mut seen_shared_ref_types: HashSet<String> = HashSet::new();
            let mut seen_mut_ref_types: HashSet<String> = HashSet::new();
            for param in &fd.params {
                if let Ty::Ref(false, inner) = resolve(&param.ty) {
                    seen_shared_ref_types.insert(inner.display());
                }
            }
            for param in &fd.params {
                if let Ty::Ref(true, inner) = resolve(&param.ty) {
                    let key = inner.display();
                    if seen_shared_ref_types.contains(&key) {
                        self.emit(CheckError::AliasingMutableBorrow {
                            name: param.name.clone(),
                            span: param.span,
                        });
                    } else if !seen_mut_ref_types.insert(key) {
                        self.emit(CheckError::DoubleMutableBorrow {
                            name: param.name.clone(),
                            span: param.span,
                        });
                    }
                }
            }
        }

        self.env.push_scope();
        for param in &fd.params {
            let ty = resolve(&param.ty);
            // Strip ref/val wrapper so the param's env type is T not ref T.
            let env_ty = match &ty {
                crate::mvl::checker::types::Ty::Ref(_, inner) => (**inner).clone(),
                _ => ty.clone(),
            };
            self.env.define(
                param.name.clone(),
                VarInfo::new(
                    env_ty,
                    matches!(ty.base(), crate::mvl::checker::types::Ty::Ref(true, _))
                        || matches!(
                            param.capability,
                            Some(crate::mvl::parser::ast::Capability::Ref)
                                | Some(crate::mvl::parser::ast::Capability::Iso)
                        ),
                )
                .with_capability(param.capability.clone()),
            );
        }

        // Use infer_block_type so that the last expression in the body is
        // treated as the implicit return value rather than a discarded statement.
        // This prevents false ResultIgnored errors for `Ok(...)` / `Err(...)`
        // at the end of Result-returning functions.
        self.infer_block_type(&fd.body, Some(&ret_ty));
        self.env.pop_scope();

        self.current_return_ty = prev_ret;
        self.current_fn_name = prev_fn_name;
        self.current_fn_effects = prev_effects;
        self.current_fn_totality = prev_totality;
        self.current_type_params = prev_type_params;
        self.current_type_constraints = prev_type_constraints;
    }

    fn check_const_decl(&mut self, cd: &ConstDecl) {
        let expected = resolve(&cd.ty);
        let found = self.infer_expr(&cd.value);
        if !types_compatible(&expected, &found) {
            self.emit(CheckError::TypeMismatch {
                expected: expected.display(),
                found: found.display(),
                span: cd.value.span(),
            });
        }
    }
}
