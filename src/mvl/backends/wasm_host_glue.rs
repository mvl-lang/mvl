// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Generates the wasmtime-embedding native host that makes `extern "rust"`
//! calls actually runnable under `--backend=wasm` (#2049).
//!
//! `wasm_text.rs` declares `(import "extern" "<name>" ...)` for every
//! `extern "rust"` fn (the direct #2049 fix), but something has to satisfy
//! those imports at run time. This module generates a small Rust `main.rs`
//! that:
//!
//! 1. Embeds `wasmtime`, loads `mvl_runtime_wasm.wasm` (the same runtime
//!    module `wasmtime run --preload runtime=...` already uses) and the
//!    compiled guest `.wasm`.
//! 2. Emits, for every user struct/refinement-newtype/unit-enum reachable
//!    from an extern fn's signature, the *same* Rust type declaration the
//!    Rust backend would generate (via `rust::emitter::RustEmitter` —
//!    reused, not reimplemented, so `bridge.rs`'s `Config { port: Port(v),
//!    .. }`-style construction sees exactly the types it already expects)
//!    plus a matching pair of `read_T`/`write_T` marshalling functions.
//! 3. Registers one `Linker::func_wrap("extern", "<name>", ...)` per extern
//!    fn. Each closure marshals its WASM-shaped arguments (the same
//!    convention `wasm_text.rs::extern_fn_signature` declares) into native
//!    Rust values, calls the *unmodified* `bridge::<name>` (the exact
//!    function `examples/*/bridge.rs` already implements for the Rust
//!    backend — `mod bridge;` links it straight into this generated crate,
//!    mirroring `cli/build.rs`'s existing Rust-backend bridge injection),
//!    and marshals the result back.
//! 4. Instantiates the guest module and calls the requested export --
//!    `_start` by default (run the program), or a `test fn` name (mirrors
//!    `wasmtime run --invoke`/`lli <ir> <test_name>` for the extern-free
//!    path, letting `test fn` files that also use extern "rust" run the
//!    same way as any other test fn).
//!
//! `bridge.rs` never changes between backends: only this glue (compiler-
//! generated, never hand-written) knows about the WASM linear-memory
//! boundary.
//!
//! Supported shapes: `Unit`, `Int`, `Bool`, `String`, user structs, unit-only
//! enums (all variants field-less — matches `wasm_text.rs`'s own
//! `collect_enums`/`collect_payload_enums` split), refinement-newtype
//! aliases (`type Port = Int where ...`), `Option[T]`, and `Result[T, E]`
//! nested arbitrarily over the above. Payload-carrying enums, `List`/`Map`/
//! `Set`, and function values are not supported yet — an extern fn whose
//! signature needs one is reported via [`UnsupportedExternFn`] rather than
//! silently mis-marshalled.

use std::collections::{HashMap, HashSet};

use crate::mvl::backends::rust::emitter::RustEmitter;
use crate::mvl::backends::wasm_text::{collect_structs, collect_type_aliases, StructLayout};
use crate::mvl::checker::types::Ty;
use crate::mvl::ir::{
    TirExternFn, TirFieldDecl, TirProgram, TirTypeBody, TirTypeDecl, TirVariantFields,
};

/// A value's shape at the WASM import boundary, and how to marshal it.
/// Mirrors `wasm_text.rs`'s own `wasm_ty`/`is_i32`/`extern_fn_signature`
/// conventions — this is the native-Rust-host side of the same ABI.
#[derive(Debug, Clone, PartialEq)]
enum Shape {
    Unit,
    Int,
    Bool,
    String,
    /// All-unit-variant enum — an `i32` discriminant, no heap allocation
    /// (matches `wasm_text.rs::collect_enums`, not `collect_payload_enums`).
    UnitEnum(String),
    /// Heap-boxed `i32` pointer, fields at `StructLayout`'s offsets.
    Struct(String),
    /// `type Name = <inner> where ...` — a newtype wrapper the Rust backend
    /// generates as `pub struct Name(pub <inner>);` (`emit_tir_alias`).
    /// Constructed/destructured as `Name(v)` / `v.0`; the WASM-side
    /// representation is just `inner`'s.
    RefinedAlias(String, Box<Shape>),
    /// `Secret[T]`/`Tainted[T]`/`Clean[T]`/`Public[T]` — zero runtime
    /// representation at the WASM boundary (peeled everywhere in
    /// `wasm_text.rs`), but a REAL `mvl_runtime::prelude::{Secret,Tainted,
    /// Clean,Public}<T>` newtype on the native Rust side, since bridge.rs's
    /// actual signatures use it directly (e.g. `init_auth_store(api_key:
    /// Secret<String>)`, `Config { api_key: Secret<String>, .. }`).
    /// Constructed/destructured exactly like `RefinedAlias` — `Label(v)` /
    /// `v.0` — the label newtypes are `#[repr(transparent)] pub struct
    /// Label<T>(pub T);`, same shape as a refinement newtype.
    Labeled(String, Box<Shape>),
    OptionOf(Box<Shape>),
    ResultOf(Box<Shape>, Box<Shape>),
}

/// Registry of every struct/unit-enum/alias the program declares, built
/// once per `generate_host_main` call and threaded through `classify`.
struct Catalog {
    structs: HashMap<String, StructLayout>,
    unit_enums: HashMap<String, Vec<String>>,
    type_aliases: HashMap<String, Ty>,
    types_by_name: HashMap<String, TirTypeDecl>,
}

impl Catalog {
    /// `extra_types` — type declarations from sibling `.mvl` files that
    /// this specific compilation didn't transitively merge in, but that
    /// `bridge.rs`'s own blanket `use crate::{...}` may still reference
    /// (bridge.rs is written for the whole example, not scoped per file —
    /// #2049 follow-up, found running `handler_test.mvl`: it never
    /// references `config.mvl`, so `Config`/`Port`/etc. are simply absent
    /// from *its* merged TIR even though bridge.rs needs them declared).
    /// Caller-supplied types are added only if `tir.types` doesn't already
    /// have that name (its own merge wins on conflict).
    fn build(tir: &TirProgram, extra_types: &[TirTypeDecl]) -> Self {
        let all_types: Vec<TirTypeDecl> = tir
            .types
            .iter()
            .cloned()
            .chain(extra_types.iter().cloned())
            .collect();
        let type_aliases = collect_type_aliases(&all_types);
        let structs = collect_structs(&all_types, &type_aliases);
        let mut unit_enums = HashMap::new();
        let mut types_by_name = HashMap::new();
        for td in &all_types {
            types_by_name
                .entry(td.name.clone())
                .or_insert_with(|| td.clone());
            if let TirTypeBody::Enum(variants) = &td.body {
                if variants
                    .iter()
                    .all(|v| matches!(v.fields, TirVariantFields::Unit))
                {
                    unit_enums
                        .entry(td.name.clone())
                        .or_insert_with(|| variants.iter().map(|v| v.name.clone()).collect());
                }
            }
        }
        Self {
            structs,
            unit_enums,
            type_aliases,
            types_by_name,
        }
    }
}

/// Peel `Ref`/`Refined` wrappers (both erase to their inner type with no
/// native-Rust-side consequence), preserve `Labeled` as `Shape::Labeled`
/// (real Rust newtype on that side, see the variant doc), and classify the
/// remaining type against the catalog. `None` means this type isn't
/// supported at the extern boundary yet (payload-carrying enum, `List`/
/// `Map`/`Set`, `Fn`) — the caller turns that into a clear diagnostic rather
/// than generating glue that would silently mis-marshal.
fn classify(ty: &Ty, cat: &Catalog) -> Option<Shape> {
    match ty {
        Ty::Ref(_, inner) | Ty::Refined(inner, _) => classify(inner, cat),
        // Unlike `Ref`/`Refined`, a label is NOT transparent on the native
        // Rust side — bridge.rs's actual signatures use `Secret<T>`/
        // `Tainted<T>`/etc. directly (#2049 follow-up: found running
        // `handler_test.mvl`'s `put_config_value`, whose bridge.rs
        // implementation takes `value: Clean<String>`). WASM-side layout
        // still treats it as fully transparent (zero representation), so
        // this only affects how the *native Rust* value is constructed/
        // destructured, not the WASM signature (`shape_is_i32`/
        // `is_string_like` below both delegate straight through).
        Ty::Labeled(label, inner) => Some(Shape::Labeled(
            label.clone(),
            Box::new(classify(inner, cat)?),
        )),
        Ty::Unit => Some(Shape::Unit),
        Ty::Int | Ty::UInt => Some(Shape::Int),
        Ty::Bool => Some(Shape::Bool),
        Ty::String => Some(Shape::String),
        Ty::Option(inner) => Some(Shape::OptionOf(Box::new(classify(inner, cat)?))),
        Ty::Result(ok, err) => Some(Shape::ResultOf(
            Box::new(classify(ok, cat)?),
            Box::new(classify(err, cat)?),
        )),
        Ty::Named(name, args) if args.is_empty() => {
            if let Some(target) = cat.type_aliases.get(name.as_str()) {
                return if let Ty::Refined(inner, _) = target {
                    Some(Shape::RefinedAlias(
                        name.clone(),
                        Box::new(classify(inner, cat)?),
                    ))
                } else {
                    // Plain (non-refined) alias, e.g. `type Foo = Int` — the
                    // Rust backend emits `pub type Foo = Int;`, a genuine
                    // synonym, so no wrapper/unwrap is needed.
                    classify(target, cat)
                };
            }
            if cat.structs.contains_key(name.as_str()) {
                return Some(Shape::Struct(name.clone()));
            }
            if cat.unit_enums.contains_key(name.as_str()) {
                return Some(Shape::UnitEnum(name.clone()));
            }
            None
        }
        _ => None,
    }
}

/// `true` when this shape occupies a single WASM `i32` (vs `i64`) —
/// determines which of the runtime's `_mvl_option_*`/`_mvl_result_*` `_i32`/
/// `_i64` function pairs to call, mirroring `wasm_text.rs::is_i32`.
fn shape_is_i32(shape: &Shape) -> bool {
    match shape {
        Shape::Int => false,
        Shape::RefinedAlias(_, inner) | Shape::Labeled(_, inner) => shape_is_i32(inner),
        Shape::Unit
        | Shape::Bool
        | Shape::String
        | Shape::UnitEnum(_)
        | Shape::Struct(_)
        | Shape::OptionOf(_)
        | Shape::ResultOf(_, _) => true,
    }
}

/// `true` when this shape's WASM-side encoding is the top-level String
/// convention (a `(ptr, len)` i32 pair, not a boxed pointer) — peels
/// `Labeled`/`RefinedAlias` wrappers, which are invisible at the WASM
/// boundary, to find the underlying base.
fn is_string_like(shape: &Shape) -> bool {
    match shape {
        Shape::String => true,
        Shape::Labeled(_, inner) | Shape::RefinedAlias(_, inner) => is_string_like(inner),
        _ => false,
    }
}

/// Collect the names of every struct/unit-enum/alias this shape directly
/// mentions (not transitively through a struct's own fields — the caller
/// walks those separately once the referenced type's declaration is in
/// hand). `Labeled` contributes no name — `Secret`/`Tainted`/`Clean`/
/// `Public` come from `mvl_runtime::prelude`, not a type this program
/// declares.
fn shape_type_refs(shape: &Shape, out: &mut HashSet<String>) {
    match shape {
        Shape::Struct(name) | Shape::UnitEnum(name) => {
            out.insert(name.clone());
        }
        Shape::RefinedAlias(name, inner) => {
            out.insert(name.clone());
            shape_type_refs(inner, out);
        }
        Shape::Labeled(_, inner) => shape_type_refs(inner, out),
        Shape::OptionOf(inner) => shape_type_refs(inner, out),
        Shape::ResultOf(ok, err) => {
            shape_type_refs(ok, out);
            shape_type_refs(err, out);
        }
        Shape::Unit | Shape::Int | Shape::Bool | Shape::String => {}
    }
}

/// Reason an extern fn (or a type it transitively needs) can't be
/// marshalled yet, with enough context to point at the offending signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedExternFn {
    pub fn_name: String,
    pub detail: String,
}

impl std::fmt::Display for UnsupportedExternFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "extern \"rust\" fn `{}` is not yet supported under --backend=wasm: {}",
            self.fn_name, self.detail
        )
    }
}

/// Transitively expand `seed` (type names referenced directly by extern
/// signatures) to every struct/alias/enum they reach through struct fields,
/// classifying each field along the way. `Err` names the first
/// unclassifiable field found, attributed to `fn_name` for the diagnostic.
fn compute_type_closure(
    seed: HashSet<String>,
    cat: &Catalog,
    fn_name: &str,
) -> Result<HashSet<String>, String> {
    let mut closure = HashSet::new();
    let mut queue: Vec<String> = seed.into_iter().collect();
    while let Some(name) = queue.pop() {
        if !closure.insert(name.clone()) {
            continue;
        }
        let Some(td) = cat.types_by_name.get(&name) else {
            continue;
        };
        if let TirTypeBody::Struct { fields, .. } = &td.body {
            for f in fields {
                let shape = classify(&f.ty, cat).ok_or_else(|| {
                    format!(
                        "field `{}` of struct `{name}` (reached from `{fn_name}`'s signature) \
                         has an unsupported type for the extern-wasm host glue",
                        f.name
                    )
                })?;
                let mut refs = HashSet::new();
                shape_type_refs(&shape, &mut refs);
                queue.extend(refs);
            }
        }
    }
    Ok(closure)
}

/// Generate the host `main.rs` source for every `extern "rust"` fn in
/// `tir.externs`. Returns `Ok(None)` when there are none (callers skip
/// building the host crate entirely and keep the plain
/// `wasmtime run --preload runtime=...` path). Returns `Err` listing every
/// fn (or transitively-needed type) whose signature isn't marshallable yet,
/// so a partial/wrong build never silently ships.
///
/// `extra_types` — type declarations from sibling `.mvl` files this
/// compilation didn't itself transitively merge in; see `Catalog::build`.
/// Pass `&[]` when the caller hasn't gathered any (e.g. no `bridge.rs` is
/// even present yet to need them).
pub fn generate_host_main(
    tir: &TirProgram,
    extra_types: &[TirTypeDecl],
) -> Result<Option<String>, Vec<UnsupportedExternFn>> {
    let externs: Vec<&TirExternFn> = tir
        .externs
        .iter()
        .filter(|ed| ed.abi == "rust")
        .flat_map(|ed| ed.fns.iter())
        .collect();
    if externs.is_empty() {
        return Ok(None);
    }

    let cat = Catalog::build(tir, extra_types);
    let mut unsupported = Vec::new();
    let mut closures = String::new();
    let mut seed_types: HashSet<String> = HashSet::new();

    // Classify every signature first (collecting all failures before
    // generating anything) and seed the type closure from the shapes that
    // succeeded.
    let mut ok_fns: Vec<(&TirExternFn, Vec<Shape>, Shape)> = Vec::new();
    for ef in &externs {
        match classify_signature(ef, &cat) {
            Ok((param_shapes, ret_shape)) => {
                for s in &param_shapes {
                    shape_type_refs(s, &mut seed_types);
                }
                shape_type_refs(&ret_shape, &mut seed_types);
                ok_fns.push((ef, param_shapes, ret_shape));
            }
            Err(detail) => unsupported.push(UnsupportedExternFn {
                fn_name: ef.name.clone(),
                detail,
            }),
        }
    }
    // `extra_types` are seeded unconditionally, not just when an extern
    // signature happens to reach them: they exist specifically because
    // bridge.rs's own `use crate::{...}` needs them declared, independent
    // of whether *this* file's own externs reference them at all (see
    // `directory_type_decls_if_rust_externs` in cli/wasm_text.rs).
    for td in extra_types {
        seed_types.insert(td.name.clone());
    }

    let type_closure = match compute_type_closure(seed_types, &cat, "<transitive field>") {
        Ok(names) => names,
        Err(detail) => {
            unsupported.push(UnsupportedExternFn {
                fn_name: "<transitive field>".to_string(),
                detail,
            });
            HashSet::new()
        }
    };

    if !unsupported.is_empty() {
        return Err(unsupported);
    }

    let type_decls = emit_type_declarations(&type_closure, &cat);
    let type_helpers = emit_type_helpers(&type_closure, &cat);

    for (ef, param_shapes, ret_shape) in &ok_fns {
        closures.push_str(&emit_closure(ef, param_shapes, ret_shape));
    }

    Ok(Some(format!(
        r#"// Auto-generated by `mvl build/test --backend=wasm` (#2049) --
// the extern "rust" FFI host. Do not edit by hand; re-run the build.
//
// Loads the compiled guest module and the MVL WASM runtime, and implements
// every `extern "rust"` import by marshalling arguments/results across the
// linear-memory boundary and calling straight into `bridge::<name>` --
// linked into this binary unmodified, exactly as the Rust backend already
// does.
#![allow(unused_mut, unused_variables, dead_code)]

mod bridge;

// `Secret`/`Tainted`/`Clean`/`Public` newtype construction/destructuring
// (`Shape::Labeled`) references these bare -- same import bridge.rs itself
// uses, but bridge.rs's `use` is scoped to its own module, not the crate
// root where this generated glue lives.
use mvl_runtime::prelude::*;
use wasmtime::{{Caller, Engine, Linker, Module, Store}};
use wasmtime_wasi::preview1::{{self, WasiP1Ctx}};
use wasmtime_wasi::WasiCtxBuilder;

struct HostState {{
    wasi: WasiP1Ctx,
}}

// ── Primitive marshalling helpers (fixed, not per-signature generated) ──

fn read_i32_at(caller: &mut Caller<'_, HostState>, ptr: i32, offset: u32) -> i32 {{
    let memory = caller.get_export("memory").and_then(|e| e.into_memory()).expect("guest module exports memory");
    let off = (ptr as usize) + (offset as usize);
    i32::from_le_bytes(memory.data(&caller)[off..off + 4].try_into().unwrap())
}}

fn read_i64_at(caller: &mut Caller<'_, HostState>, ptr: i32, offset: u32) -> i64 {{
    let memory = caller.get_export("memory").and_then(|e| e.into_memory()).expect("guest module exports memory");
    let off = (ptr as usize) + (offset as usize);
    i64::from_le_bytes(memory.data(&caller)[off..off + 8].try_into().unwrap())
}}

fn write_i32_at(caller: &mut Caller<'_, HostState>, ptr: i32, offset: u32, v: i32) {{
    let memory = caller.get_export("memory").and_then(|e| e.into_memory()).expect("guest module exports memory");
    let off = (ptr as usize) + (offset as usize);
    memory.write(&mut *caller, off, &v.to_le_bytes()).expect("write i32 into guest memory");
}}

fn write_i64_at(caller: &mut Caller<'_, HostState>, ptr: i32, offset: u32, v: i64) {{
    let memory = caller.get_export("memory").and_then(|e| e.into_memory()).expect("guest module exports memory");
    let off = (ptr as usize) + (offset as usize);
    memory.write(&mut *caller, off, &v.to_le_bytes()).expect("write i64 into guest memory");
}}

fn alloc_bytes(caller: &mut Caller<'_, HostState>, runtime_instance: wasmtime::Instance, size: i32) -> i32 {{
    let alloc = runtime_instance.get_typed_func::<i32, i32>(&mut *caller, "_mvl_struct_alloc").expect("runtime module exports _mvl_struct_alloc");
    alloc.call(&mut *caller, size).expect("_mvl_struct_alloc call succeeds")
}}

/// Read a `(ptr, len)` MVL string argument out of the guest's shared memory.
fn read_mvl_string(caller: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> String {{
    let memory = caller.get_export("memory").and_then(|e| e.into_memory()).expect("guest module exports memory");
    let bytes = memory.data(&caller)[ptr as usize..(ptr + len) as usize].to_vec();
    String::from_utf8(bytes).expect("MVL string is valid UTF-8")
}}

/// Write a native `String` result into the guest's shared memory and return
/// its `(ptr, len)` -- the same flat representation an ordinary MVL
/// function returns, no `*MvlString` box involved at this boundary.
fn write_mvl_string(caller: &mut Caller<'_, HostState>, runtime_instance: wasmtime::Instance, s: &str) -> (i32, i32) {{
    let bytes = s.as_bytes();
    let ptr = alloc_bytes(&mut *caller, runtime_instance, bytes.len() as i32);
    let memory = caller.get_export("memory").and_then(|e| e.into_memory()).expect("guest module exports memory");
    memory.write(&mut *caller, ptr as usize, bytes).expect("write string bytes into guest memory");
    (ptr, bytes.len() as i32)
}}

/// A String nested inside a struct field/Option/Result payload is a boxed
/// `*MvlString` (ptr @ offset 0, len @ offset 4) -- unlike a top-level
/// param/return, which is a flat `(ptr, len)` pair.
fn read_mvl_string_boxed(caller: &mut Caller<'_, HostState>, ms_ptr: i32) -> String {{
    let ptr = read_i32_at(&mut *caller, ms_ptr, 0);
    let len = read_i32_at(&mut *caller, ms_ptr, 4);
    read_mvl_string(&mut *caller, ptr, len)
}}

fn write_mvl_string_boxed(caller: &mut Caller<'_, HostState>, runtime_instance: wasmtime::Instance, s: &str) -> i32 {{
    let bytes = s.as_bytes();
    let scratch = alloc_bytes(&mut *caller, runtime_instance, bytes.len() as i32);
    let memory = caller.get_export("memory").and_then(|e| e.into_memory()).expect("guest module exports memory");
    memory.write(&mut *caller, scratch as usize, bytes).expect("write string bytes into guest memory");
    let ctor = runtime_instance.get_typed_func::<(i32, i32), i32>(&mut *caller, "_mvl_string_new").expect("runtime module exports _mvl_string_new");
    ctor.call(&mut *caller, (scratch, bytes.len() as i32)).expect("_mvl_string_new call succeeds")
}}

fn option_tag(caller: &mut Caller<'_, HostState>, runtime_instance: wasmtime::Instance, opt: i32) -> i32 {{
    runtime_instance.get_typed_func::<i32, i32>(&mut *caller, "_mvl_option_tag").expect("_mvl_option_tag").call(&mut *caller, opt).expect("_mvl_option_tag call")
}}
fn option_value_i32(caller: &mut Caller<'_, HostState>, runtime_instance: wasmtime::Instance, opt: i32) -> i32 {{
    runtime_instance.get_typed_func::<i32, i32>(&mut *caller, "_mvl_option_value_i32").expect("_mvl_option_value_i32").call(&mut *caller, opt).expect("_mvl_option_value_i32 call")
}}
fn option_value_i64(caller: &mut Caller<'_, HostState>, runtime_instance: wasmtime::Instance, opt: i32) -> i64 {{
    runtime_instance.get_typed_func::<i32, i64>(&mut *caller, "_mvl_option_value_i64").expect("_mvl_option_value_i64").call(&mut *caller, opt).expect("_mvl_option_value_i64 call")
}}
fn option_some_i32(caller: &mut Caller<'_, HostState>, runtime_instance: wasmtime::Instance, v: i32) -> i32 {{
    runtime_instance.get_typed_func::<i32, i32>(&mut *caller, "_mvl_option_some_i32").expect("_mvl_option_some_i32").call(&mut *caller, v).expect("_mvl_option_some_i32 call")
}}
fn option_some_i64(caller: &mut Caller<'_, HostState>, runtime_instance: wasmtime::Instance, v: i64) -> i32 {{
    runtime_instance.get_typed_func::<i64, i32>(&mut *caller, "_mvl_option_some_i64").expect("_mvl_option_some_i64").call(&mut *caller, v).expect("_mvl_option_some_i64 call")
}}
fn option_none(caller: &mut Caller<'_, HostState>, runtime_instance: wasmtime::Instance) -> i32 {{
    runtime_instance.get_typed_func::<(), i32>(&mut *caller, "_mvl_option_none").expect("_mvl_option_none").call(&mut *caller, ()).expect("_mvl_option_none call")
}}
fn result_tag(caller: &mut Caller<'_, HostState>, runtime_instance: wasmtime::Instance, r: i32) -> i32 {{
    runtime_instance.get_typed_func::<i32, i32>(&mut *caller, "_mvl_result_tag").expect("_mvl_result_tag").call(&mut *caller, r).expect("_mvl_result_tag call")
}}
fn result_value_i32(caller: &mut Caller<'_, HostState>, runtime_instance: wasmtime::Instance, r: i32) -> i32 {{
    runtime_instance.get_typed_func::<i32, i32>(&mut *caller, "_mvl_result_value_i32").expect("_mvl_result_value_i32").call(&mut *caller, r).expect("_mvl_result_value_i32 call")
}}
fn result_value_i64(caller: &mut Caller<'_, HostState>, runtime_instance: wasmtime::Instance, r: i32) -> i64 {{
    runtime_instance.get_typed_func::<i32, i64>(&mut *caller, "_mvl_result_value_i64").expect("_mvl_result_value_i64").call(&mut *caller, r).expect("_mvl_result_value_i64 call")
}}
fn result_ok_i32(caller: &mut Caller<'_, HostState>, runtime_instance: wasmtime::Instance, v: i32) -> i32 {{
    runtime_instance.get_typed_func::<i32, i32>(&mut *caller, "_mvl_result_ok_i32").expect("_mvl_result_ok_i32").call(&mut *caller, v).expect("_mvl_result_ok_i32 call")
}}
fn result_ok_i64(caller: &mut Caller<'_, HostState>, runtime_instance: wasmtime::Instance, v: i64) -> i32 {{
    runtime_instance.get_typed_func::<i64, i32>(&mut *caller, "_mvl_result_ok_i64").expect("_mvl_result_ok_i64").call(&mut *caller, v).expect("_mvl_result_ok_i64 call")
}}
fn result_err_i32(caller: &mut Caller<'_, HostState>, runtime_instance: wasmtime::Instance, v: i32) -> i32 {{
    runtime_instance.get_typed_func::<i32, i32>(&mut *caller, "_mvl_result_err_i32").expect("_mvl_result_err_i32").call(&mut *caller, v).expect("_mvl_result_err_i32 call")
}}
fn result_err_i64(caller: &mut Caller<'_, HostState>, runtime_instance: wasmtime::Instance, v: i64) -> i32 {{
    runtime_instance.get_typed_func::<i64, i32>(&mut *caller, "_mvl_result_err_i64").expect("_mvl_result_err_i64").call(&mut *caller, v).expect("_mvl_result_err_i64 call")
}}

// ── Generated type declarations (mirrors the Rust backend's own codegen) ──

{type_decls}

// ── Generated per-type read_T/write_T marshalling helpers ──

{type_helpers}

fn main() -> anyhow::Result<()> {{
    let mut args = std::env::args().skip(1);
    let runtime_path = args
        .next()
        .expect("usage: <host> <runtime.wasm> <guest.wasm> [invoke_fn]");
    let guest_path = args
        .next()
        .expect("usage: <host> <runtime.wasm> <guest.wasm> [invoke_fn]");
    // Defaults to `_start` (run the program) -- pass a `test fn` name
    // instead to invoke it directly, mirroring `wasmtime run --invoke` /
    // `lli <ir> <test_name>` for the plain (extern-free) path.
    let invoke = args.next().unwrap_or_else(|| "_start".to_string());

    let engine = Engine::default();
    let runtime_module = Module::from_file(&engine, &runtime_path)?;
    let guest_module = Module::from_file(&engine, &guest_path)?;

    let wasi = WasiCtxBuilder::new()
        .inherit_stdio()
        .inherit_env()
        .inherit_args()
        .build_p1();
    let mut store = Store::new(&engine, HostState {{ wasi }});

    let mut linker: Linker<HostState> = Linker::new(&engine);
    preview1::add_to_linker_sync(&mut linker, |s: &mut HostState| &mut s.wasi)?;

    let runtime_instance = linker.instantiate(&mut store, &runtime_module)?;
    linker.instance(&mut store, "runtime", runtime_instance)?;

{closures}
    let guest_instance = linker.instantiate(&mut store, &guest_module)?;
    let entry = guest_instance.get_typed_func::<(), ()>(&mut store, &invoke)?;
    entry.call(&mut store, ())?;
    Ok(())
}}
"#
    )))
}

/// Classify an extern fn's full signature. `Err` names the first
/// unsupported param (by name) or the return type.
fn classify_signature(ef: &TirExternFn, cat: &Catalog) -> Result<(Vec<Shape>, Shape), String> {
    let mut param_shapes = Vec::with_capacity(ef.params.len());
    for p in &ef.params {
        match classify(&p.ty, cat) {
            Some(s) => param_shapes.push(s),
            None => {
                return Err(format!(
                    "param `{}` has an unsupported type for the extern-wasm host glue",
                    p.name
                ))
            }
        }
    }
    let ret_shape = classify(&ef.ret_ty, cat).ok_or_else(|| {
        "return type has an unsupported type for the extern-wasm host glue".to_string()
    })?;
    Ok((param_shapes, ret_shape))
}

/// Generate `pub struct`/`pub enum` declarations for every type in the
/// closure, reusing the Rust backend's own emitter (`RustEmitter::
/// emit_tir_type_decl`) so a struct like `Config` gets *exactly* the same
/// Rust type `bridge.rs` already targets for the Rust backend — including
/// refinement-newtype wrappers (`pub struct Port(pub i64);`) and their
/// validating `::new()` constructors.
fn emit_type_declarations(closure: &HashSet<String>, cat: &Catalog) -> String {
    let mut names: Vec<&String> = closure.iter().collect();
    names.sort();
    let mut emitter = RustEmitter::new();
    for name in names {
        if let Some(td) = cat.types_by_name.get(name) {
            emitter.emit_tir_type_decl(td);
        }
    }
    emitter.finish()
}

/// Generate `read_T`/`write_T` marshalling functions for every struct and
/// unit-enum in the closure (aliases need no helper — they marshal via the
/// wrapped shape directly, see `Shape::RefinedAlias`).
fn emit_type_helpers(closure: &HashSet<String>, cat: &Catalog) -> String {
    let mut names: Vec<&String> = closure.iter().collect();
    names.sort();
    let mut out = String::new();
    for name in names {
        let Some(td) = cat.types_by_name.get(name) else {
            continue;
        };
        match &td.body {
            TirTypeBody::Struct { fields, .. } => {
                out.push_str(&emit_struct_helpers(name, fields, cat));
            }
            TirTypeBody::Enum(_) if cat.unit_enums.contains_key(name.as_str()) => {
                out.push_str(&emit_unit_enum_helpers(
                    name,
                    &cat.unit_enums[name.as_str()],
                ));
            }
            _ => {}
        }
    }
    out
}

fn emit_unit_enum_helpers(name: &str, variants: &[String]) -> String {
    let read_arms: String = variants
        .iter()
        .enumerate()
        .map(|(i, v)| format!("        {i} => {name}::{v},\n"))
        .collect();
    let write_arms: String = variants
        .iter()
        .enumerate()
        .map(|(i, v)| format!("        {name}::{v} => {i},\n"))
        .collect();
    format!(
        "fn read_{name}(disc: i32) -> {name} {{\n    match disc {{\n{read_arms}        _ => panic!(\"unknown {name} discriminant: {{disc}}\"),\n    }}\n}}\n\
         fn write_{name}(v: {name}) -> i32 {{\n    match v {{\n{write_arms}    }}\n}}\n\n"
    )
}

fn emit_struct_helpers(name: &str, fields: &[TirFieldDecl], cat: &Catalog) -> String {
    // Guaranteed present: `name` only reaches here via the type closure,
    // which only queues struct names once `collect_structs` has already
    // produced a layout for them.
    let layout = &cat.structs[name];
    let mut read_lets = String::new();
    let mut field_inits = Vec::with_capacity(fields.len());
    let mut write_lets = String::new();
    let mut write_stmts = String::new();
    for (i, (field, slot)) in fields.iter().zip(layout.fields.iter()).enumerate() {
        // Every field type here already passed `classify` during
        // `compute_type_closure` — safe to unwrap.
        let shape =
            classify(&field.ty, cat).expect("field type classified during closure computation");
        let is_i32 = shape_is_i32(&shape);

        // A raw memory read/write is itself a call needing `&mut *caller`.
        // Nesting it directly as an argument to another such call (e.g.
        // `read_mvl_string_boxed(&mut *caller, read_i32_at(&mut *caller,
        // ...))`) does NOT compile: Rust holds the outer call's `&mut
        // *caller` reborrow live across evaluating its own arguments, so a
        // second reborrow of the same place in the argument list conflicts
        // (E0499). Bind each raw value to its own `let` first, then pass the
        // *name* — a proven pattern, not a Rust corner case; wasmtime's own
        // examples reborrow the same way for sequential calls.
        let raw = format!("__f{i}");
        if is_i32 {
            read_lets.push_str(&format!(
                "    let {raw} = read_i32_at(&mut *caller, ptr, {});\n",
                slot.offset
            ));
        } else {
            read_lets.push_str(&format!(
                "    let {raw} = read_i64_at(&mut *caller, ptr, {});\n",
                slot.offset
            ));
        }
        field_inits.push(format!("{}: {}", field.name, read_expr_boxed(&shape, &raw)));

        let written = format!("__w{i}");
        let write_val = write_expr_boxed(&shape, &format!("v.{}", field.name));
        write_lets.push_str(&format!("    let {written} = {write_val};\n"));
        if is_i32 {
            write_stmts.push_str(&format!(
                "    write_i32_at(&mut *caller, ptr, {}, {written});\n",
                slot.offset
            ));
        } else {
            write_stmts.push_str(&format!(
                "    write_i64_at(&mut *caller, ptr, {}, {written});\n",
                slot.offset
            ));
        }
    }

    format!(
        "fn read_{name}(caller: &mut Caller<'_, HostState>, runtime_instance: wasmtime::Instance, ptr: i32) -> {name} {{\n{read_lets}    {name} {{\n        {}\n    }}\n}}\n\
         fn write_{name}(caller: &mut Caller<'_, HostState>, runtime_instance: wasmtime::Instance, v: {name}) -> i32 {{\n    let ptr = alloc_bytes(&mut *caller, runtime_instance, {});\n{write_lets}{write_stmts}    ptr\n}}\n\n",
        field_inits.join(",\n        "),
        layout.total_size,
    )
}

/// Read a top-level string-like param (`is_string_like(shape)` — a plain
/// `String`, or `Secret[String]`/a refined-String-alias wrapping one) from
/// its `(ptr, len)` WASM params: unlike a *nested* String (boxed
/// `*MvlString`, one `i32`), a top-level String is the flat `(ptr, len)`
/// pair `wasm_text.rs::extern_fn_signature` declares. Peels
/// `Labeled`/`RefinedAlias` wrappers, rewrapping the resulting native
/// `String` at each level on the way back out.
fn read_top_level_string(shape: &Shape, ptr_expr: &str, len_expr: &str) -> String {
    match shape {
        Shape::Labeled(label, inner) => {
            format!(
                "{label}({})",
                read_top_level_string(inner, ptr_expr, len_expr)
            )
        }
        Shape::RefinedAlias(name, inner) => {
            format!(
                "{name}({})",
                read_top_level_string(inner, ptr_expr, len_expr)
            )
        }
        Shape::String => format!("read_mvl_string(&mut *caller, {ptr_expr}, {len_expr})"),
        _ => unreachable!("is_string_like(shape) must hold before calling read_top_level_string"),
    }
}

/// Given a Rust expression `wasm_expr` already evaluated to the WASM-side
/// value for `shape` (an `i32` or `i64` matching `shape_is_i32`), produce a
/// Rust expression yielding the corresponding native value. Fully recursive
/// over `Option`/`Result`/`RefinedAlias` nesting.
fn read_expr_boxed(shape: &Shape, wasm_expr: &str) -> String {
    match shape {
        Shape::Unit => "()".to_string(),
        Shape::Int => wasm_expr.to_string(),
        Shape::Bool => format!("({wasm_expr} != 0)"),
        Shape::String => format!("read_mvl_string_boxed(&mut *caller, {wasm_expr})"),
        Shape::UnitEnum(name) => format!("read_{name}({wasm_expr})"),
        Shape::Struct(name) => format!("read_{name}(&mut *caller, runtime_instance, {wasm_expr})"),
        Shape::RefinedAlias(name, inner) => {
            format!("{name}({})", read_expr_boxed(inner, wasm_expr))
        }
        Shape::Labeled(label, inner) => {
            format!("{label}({})", read_expr_boxed(inner, wasm_expr))
        }
        Shape::OptionOf(inner) => {
            let value_getter = if shape_is_i32(inner) {
                "option_value_i32"
            } else {
                "option_value_i64"
            };
            format!(
                "{{ let __opt = {wasm_expr}; if option_tag(&mut *caller, runtime_instance, __opt) == 1 {{ None }} else {{ let __v = {value_getter}(&mut *caller, runtime_instance, __opt); Some({}) }} }}",
                read_expr_boxed(inner, "__v")
            )
        }
        Shape::ResultOf(ok, err) => {
            let ok_getter = if shape_is_i32(ok) {
                "result_value_i32"
            } else {
                "result_value_i64"
            };
            let err_getter = if shape_is_i32(err) {
                "result_value_i32"
            } else {
                "result_value_i64"
            };
            format!(
                "{{ let __res = {wasm_expr}; if result_tag(&mut *caller, runtime_instance, __res) == 0 {{ Ok({{ let __v = {ok_getter}(&mut *caller, runtime_instance, __res); {} }}) }} else {{ Err({{ let __v = {err_getter}(&mut *caller, runtime_instance, __res); {} }}) }} }}",
                read_expr_boxed(ok, "__v"),
                read_expr_boxed(err, "__v"),
            )
        }
    }
}

/// Given a Rust expression `value_expr` for a native value of `shape`,
/// produce a Rust expression yielding the WASM-side `i32`/`i64` value
/// (matching `shape_is_i32`) that represents it. The inverse of
/// `read_expr_boxed`; fully recursive over the same nesting.
fn write_expr_boxed(shape: &Shape, value_expr: &str) -> String {
    match shape {
        Shape::Unit => "0".to_string(),
        Shape::Int => value_expr.to_string(),
        Shape::Bool => format!("if {value_expr} {{ 1 }} else {{ 0 }}"),
        Shape::String => {
            format!("write_mvl_string_boxed(&mut *caller, runtime_instance, &{value_expr})")
        }
        Shape::UnitEnum(name) => format!("write_{name}({value_expr})"),
        Shape::Struct(name) => {
            format!("write_{name}(&mut *caller, runtime_instance, {value_expr})")
        }
        Shape::RefinedAlias(_, inner) | Shape::Labeled(_, inner) => {
            write_expr_boxed(inner, &format!("({value_expr}).0"))
        }
        Shape::OptionOf(inner) => {
            let some_ctor = if shape_is_i32(inner) {
                "option_some_i32"
            } else {
                "option_some_i64"
            };
            format!(
                "match {value_expr} {{ Some(__v) => {{ let __w = {}; {some_ctor}(&mut *caller, runtime_instance, __w) }}, None => option_none(&mut *caller, runtime_instance) }}",
                write_expr_boxed(inner, "__v")
            )
        }
        Shape::ResultOf(ok, err) => {
            let ok_ctor = if shape_is_i32(ok) {
                "result_ok_i32"
            } else {
                "result_ok_i64"
            };
            let err_ctor = if shape_is_i32(err) {
                "result_err_i32"
            } else {
                "result_err_i64"
            };
            format!(
                "match {value_expr} {{ Ok(__v) => {{ let __w = {}; {ok_ctor}(&mut *caller, runtime_instance, __w) }}, Err(__v) => {{ let __w = {}; {err_ctor}(&mut *caller, runtime_instance, __w) }} }}",
                write_expr_boxed(ok, "__v"),
                write_expr_boxed(err, "__v"),
            )
        }
    }
}

/// Generate one `linker.func_wrap("extern", "<name>", ...)` registration.
fn emit_closure(ef: &TirExternFn, param_shapes: &[Shape], ret_shape: &Shape) -> String {
    // WASM-side closure parameter list: a top-level string-like param
    // (`String`, or `Secret[String]`/a refined-String-alias wrapping one) is
    // two i32 locals (ptr, len) -- matches
    // `wasm_text.rs::extern_fn_signature` exactly. Every other shape
    // (including nested-boxed ones) is a single i32/i64 local.
    let mut wasm_params = String::new();
    let mut native_arg_binds = String::new();
    let mut native_arg_names = Vec::with_capacity(param_shapes.len());
    for (i, shape) in param_shapes.iter().enumerate() {
        let native_name = format!("arg{i}");
        if is_string_like(shape) {
            wasm_params.push_str(&format!(", {native_name}_ptr: i32, {native_name}_len: i32"));
            let bound = read_top_level_string(
                shape,
                &format!("{native_name}_ptr"),
                &format!("{native_name}_len"),
            );
            native_arg_binds.push_str(&format!("    let {native_name} = {bound};\n"));
        } else {
            let wasm_ty = if shape_is_i32(shape) { "i32" } else { "i64" };
            wasm_params.push_str(&format!(", {native_name}: {wasm_ty}"));
            let bound = read_expr_boxed(shape, &native_name);
            if bound != native_name {
                native_arg_binds.push_str(&format!("    let {native_name} = {bound};\n"));
            }
        }
        native_arg_names.push(native_name);
    }

    let wasm_result = if is_string_like(ret_shape) {
        " -> (i32, i32)".to_string()
    } else {
        match ret_shape {
            Shape::Unit => String::new(),
            _ if shape_is_i32(ret_shape) => " -> i32".to_string(),
            _ => " -> i64".to_string(),
        }
    };

    // `.into()` on every argument: covers not just `Shape::Labeled` (which
    // already constructs the exact `Secret<T>`/etc. the callee wants — this
    // is then just a harmless identity conversion) but also bridge.rs
    // authors who use a label the MVL signature never mentions at all —
    // e.g. `put_config_value(path: String, value: String)` in MVL, but
    // `fn put_config_value(path: String, value: Clean<String>)` in
    // bridge.rs (`Clean` documents "already sanitized" purely on the Rust
    // side; MVL has no `Clean[T]` label). Mirrors the Rust backend's own
    // `emit_expr_as_value_arg(coerce: true)`, which relies on the same
    // blanket `impl<T> From<T> for Label<T>` in `mvl_runtime::ifc` (#2049
    // follow-up, found running `handler_test.mvl`'s `handle_put`).
    let call_args = native_arg_names
        .iter()
        .map(|n| format!("{n}.into()"))
        .collect::<Vec<_>>()
        .join(", ");
    let call_and_return = if is_string_like(ret_shape) {
        format!(
            "{{\n        let __result: String = bridge::{}({call_args}).into();\n        write_mvl_string(&mut *caller, runtime_instance, &__result)\n    }}",
            ef.name
        )
    } else {
        match ret_shape {
            Shape::Unit => format!("bridge::{}({call_args});", ef.name),
            _ => write_expr_boxed(ret_shape, &format!("bridge::{}({call_args})", ef.name)),
        }
    };

    format!(
        r#"    linker.func_wrap(
        "extern",
        "{name}",
        move |mut caller: Caller<'_, HostState>{wasm_params}|{wasm_result} {{
        let caller = &mut caller;
{native_arg_binds}        {call_and_return}
        }},
    )?;
"#,
        name = ef.name,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::ir::{TirExternDecl, TirParam, TirVariant};
    use crate::mvl::parser::lexer::Span;

    fn dummy_span() -> Span {
        Span {
            offset: 0,
            len: 0,
            line: 1,
            col: 1,
        }
    }

    fn param(name: &str, ty: Ty) -> TirParam {
        TirParam {
            name: name.to_string(),
            ty,
            capability: None,
            span: dummy_span(),
        }
    }

    fn extern_fn(name: &str, params: Vec<TirParam>, ret_ty: Ty) -> TirExternFn {
        TirExternFn {
            name: name.to_string(),
            params,
            ret_ty,
            effects: Vec::new(),
            totality: None,
            span: dummy_span(),
        }
    }

    fn program(fns: Vec<TirExternFn>, types: Vec<TirTypeDecl>) -> TirProgram {
        TirProgram {
            externs: vec![TirExternDecl {
                abi: "rust".to_string(),
                link_libs: Vec::new(),
                fns,
                span: dummy_span(),
            }],
            types,
            ..Default::default()
        }
    }

    fn program_with(fns: Vec<TirExternFn>) -> TirProgram {
        program(fns, Vec::new())
    }

    fn struct_decl(name: &str, fields: Vec<TirFieldDecl>) -> TirTypeDecl {
        TirTypeDecl {
            visible: true,
            name: name.to_string(),
            params: Vec::new(),
            body: TirTypeBody::Struct {
                fields,
                invariant: None,
            },
            span: dummy_span(),
        }
    }

    fn field(name: &str, ty: Ty) -> TirFieldDecl {
        TirFieldDecl {
            name: name.to_string(),
            ty,
            refinement: None,
            span: dummy_span(),
        }
    }

    fn unit_enum_decl(name: &str, variants: &[&str]) -> TirTypeDecl {
        TirTypeDecl {
            visible: true,
            name: name.to_string(),
            params: Vec::new(),
            body: TirTypeBody::Enum(
                variants
                    .iter()
                    .map(|v| TirVariant {
                        name: v.to_string(),
                        fields: TirVariantFields::Unit,
                        span: dummy_span(),
                    })
                    .collect(),
            ),
            span: dummy_span(),
        }
    }

    /// Parses `src` through the real parse/check/lower pipeline and returns
    /// the `TirTypeDecl` named `name` -- ADR-0050 forbids referencing
    /// `parser::ast` types (like `RefExpr`) anywhere under `src/mvl/
    /// backends/`, including test code (zero-tolerance budget in
    /// `tools/audit_backend_ast.py`), so a refinement predicate for `type
    /// Port = Int where ...` can't be hand-constructed here the way the
    /// rest of this module's test fixtures are -- it has to come from a
    /// real parse, same as `wasm_text.rs`'s own `compile()` test helper.
    fn parse_type_decl(src: &str, name: &str) -> TirTypeDecl {
        let (mut parser, lex_errs) = crate::mvl::parser::Parser::new(src);
        assert!(lex_errs.is_empty(), "lex errors: {lex_errs:?}");
        let prog = parser.parse_program();
        assert!(
            parser.errors().is_empty(),
            "parse errors: {:?}",
            parser.errors()
        );
        let mut expr_types = crate::mvl::checker::collect_prelude_expr_types(&[]);
        expr_types.extend(crate::mvl::checker::check(&prog).expr_types);
        let all_fns = crate::mvl::passes::mono::collect_fns([&prog]);
        let mono = crate::mvl::passes::mono::monomorphize(&prog, &all_fns, &expr_types);
        let tir = crate::mvl::ir::lower::lower(&prog, &mono, &expr_types);
        tir.types
            .into_iter()
            .find(|td| td.name == name)
            .unwrap_or_else(|| panic!("type `{name}` not found in parsed TIR for: {src}"))
    }

    #[test]
    fn no_externs_returns_none() {
        assert_eq!(
            generate_host_main(&TirProgram::default(), &[]).unwrap(),
            None
        );
    }

    #[test]
    fn int_bool_string_signature_generates_glue() {
        let tir = program_with(vec![
            extern_fn("greet", vec![param("name", Ty::String)], Ty::String),
            extern_fn("double", vec![param("n", Ty::Int)], Ty::Int),
            extern_fn("is_even", vec![param("n", Ty::Int)], Ty::Bool),
            extern_fn("log", vec![param("msg", Ty::String)], Ty::Unit),
        ]);
        let src = generate_host_main(&tir, &[]).unwrap().unwrap();
        assert!(src.contains(r#""greet""#));
        assert!(src.contains("arg0_ptr: i32, arg0_len: i32"));
        assert!(src.contains("-> (i32, i32)"));
        assert!(src.contains(r#""double""#));
        assert!(src.contains("arg0: i64"));
        assert!(src.contains("-> i64"));
        assert!(src.contains(r#""is_even""#));
        // Every argument gets `.into()` at the call site (identity for
        // already-plain values; see `labeled_string_param_gets_wrapped_and_coerced`
        // for the case where it does real work).
        assert!(src.contains("if bridge::is_even(arg0.into()) { 1 } else { 0 }"));
        assert!(src.contains(r#""log""#));
        assert!(src.contains("bridge::log(arg0.into());"));
        assert!(src.contains("mod bridge;"));
    }

    #[test]
    fn labeled_string_param_gets_wrapped_and_coerced() {
        // Secret[String]/Tainted[String] are zero-representation at the WASM
        // boundary (still the flat top-level (ptr, len) pair), but bridge.rs's
        // ACTUAL Rust signature takes `Secret<String>` directly -- the read
        // side must construct that exact newtype, not a bare String.
        let tir = program_with(vec![extern_fn(
            "init_auth_store",
            vec![param(
                "api_key",
                Ty::Labeled("Secret".to_string(), Box::new(Ty::String)),
            )],
            Ty::Unit,
        )]);
        let src = generate_host_main(&tir, &[]).unwrap().unwrap();
        assert!(src.contains("arg0_ptr: i32, arg0_len: i32"));
        assert!(
            src.contains("let arg0 = Secret(read_mvl_string(&mut *caller, arg0_ptr, arg0_len));")
        );
        assert!(src.contains("bridge::init_auth_store(arg0.into());"));
        assert!(src.contains("use mvl_runtime::prelude::*;"));
    }

    #[test]
    fn unsupported_param_type_is_reported_not_miscompiled() {
        let tir = program_with(vec![extern_fn(
            "load_config",
            vec![param("path", Ty::String)],
            Ty::Named("Config".to_string(), Vec::new()),
        )]);
        let err = generate_host_main(&tir, &[]).unwrap_err();
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].fn_name, "load_config");
        assert!(err[0].detail.contains("unsupported"));
    }

    #[test]
    fn struct_with_refined_alias_and_unit_enum_field_generates_helpers() {
        let types = vec![
            parse_type_decl("type Port = Int where self > 0\n", "Port"),
            unit_enum_decl("Method", &["Get", "Put"]),
            struct_decl(
                "Config",
                vec![
                    field("port", Ty::Named("Port".to_string(), vec![])),
                    field("method", Ty::Named("Method".to_string(), vec![])),
                    field("name", Ty::String),
                ],
            ),
        ];
        let tir = program(
            vec![extern_fn(
                "load",
                vec![param("path", Ty::String)],
                Ty::Named("Config".to_string(), vec![]),
            )],
            types,
        );
        let src = generate_host_main(&tir, &[]).unwrap().unwrap();
        assert!(src.contains("pub struct Port(pub i64);"), "{src}");
        assert!(src.contains("pub enum Method {"), "{src}");
        assert!(src.contains("pub struct Config {"), "{src}");
        assert!(src.contains("fn read_Config("), "{src}");
        assert!(src.contains("fn write_Config("), "{src}");
        assert!(src.contains("fn read_Method(disc: i32) -> Method"), "{src}");
        // Raw field reads bind to their own `let` first (avoids nesting two
        // `&mut *caller` reborrows in one call's argument list, E0499) --
        // `port: Port(__f0)` where `__f0` was bound from `read_i64_at`.
        assert!(
            src.contains("let __f0 = read_i64_at(&mut *caller, ptr,"),
            "{src}"
        );
        assert!(src.contains("port: Port(__f0)"), "{src}");
        assert!(
            src.contains("let __f1 = read_i32_at(&mut *caller, ptr,"),
            "{src}"
        );
        assert!(src.contains("method: read_Method(__f1)"), "{src}");
        // extern fn's own return: Config is i32-shaped (heap pointer).
        assert!(src.contains(r#""load""#));
        assert!(
            src.contains("write_Config(&mut *caller, runtime_instance, bridge::load(arg0.into()))")
        );
    }

    #[test]
    fn option_return_of_struct_writes_via_option_some_i32() {
        // `Option[Request]` as a RETURN value -- write direction: construct
        // the Request in guest memory, then box it with option_some_i32.
        let types = vec![struct_decl("Request", vec![field("path", Ty::String)])];
        let tir = program(
            vec![extern_fn(
                "recv",
                vec![param("index", Ty::Int)],
                Ty::Option(Box::new(Ty::Named("Request".to_string(), vec![]))),
            )],
            types,
        );
        let src = generate_host_main(&tir, &[]).unwrap().unwrap();
        assert!(
            src.contains(
                "match bridge::recv(arg0.into()) { Some(__v) => { let __w = write_Request(&mut *caller, runtime_instance, __v); option_some_i32(&mut *caller, runtime_instance, __w) }, None => option_none(&mut *caller, runtime_instance) }"
            ),
            "{src}"
        );
    }

    #[test]
    fn option_param_of_struct_reads_via_option_tag() {
        // `Option[Request]` as a PARAM -- read direction: check the tag,
        // then unbox via option_value_i32 + read_Request.
        let types = vec![struct_decl("Request", vec![field("path", Ty::String)])];
        let tir = program(
            vec![extern_fn(
                "handle",
                vec![param(
                    "req",
                    Ty::Option(Box::new(Ty::Named("Request".to_string(), vec![]))),
                )],
                Ty::Unit,
            )],
            types,
        );
        let src = generate_host_main(&tir, &[]).unwrap().unwrap();
        assert!(
            src.contains("option_tag(&mut *caller, runtime_instance, __opt) == 1"),
            "{src}"
        );
        assert!(
            src.contains("let __v = option_value_i32(&mut *caller, runtime_instance, __opt); Some(read_Request(&mut *caller, runtime_instance, __v))"),
            "{src}"
        );
        assert!(src.contains("bridge::handle(arg0.into());"), "{src}");
    }

    #[test]
    fn result_return_of_struct_and_string_err_writes_via_result_ctors() {
        // `Result[Request, String]` as a RETURN value -- write direction.
        let types = vec![struct_decl("Request", vec![field("path", Ty::String)])];
        let tir = program(
            vec![extern_fn(
                "load",
                vec![],
                Ty::Result(
                    Box::new(Ty::Named("Request".to_string(), vec![])),
                    Box::new(Ty::String),
                ),
            )],
            types,
        );
        let src = generate_host_main(&tir, &[]).unwrap().unwrap();
        assert!(
            src.contains(
                "match bridge::load() { Ok(__v) => { let __w = write_Request(&mut *caller, runtime_instance, __v); result_ok_i32(&mut *caller, runtime_instance, __w) }, Err(__v) => { let __w = write_mvl_string_boxed(&mut *caller, runtime_instance, &__v); result_err_i32(&mut *caller, runtime_instance, __w) } }"
            ),
            "{src}"
        );
    }
}
