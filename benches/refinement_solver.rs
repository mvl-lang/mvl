// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Benchmark suite for the layered refinement solver (issue #595).
//!
//! Measures per-layer performance and compares `Layered` vs `Z3Only` vs
//! `FastOnly` modes across micro-programs and corpus files.
//!
//! Run with:
//!   cargo bench --bench refinement_solver
//!   cargo bench --bench refinement_solver -- layer   # filter by name
//!
//! HTML report written to `target/criterion/`.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use mvl::mvl::checker::{check_with_two_preludes_mode, SolverMode};
use mvl::mvl::parser::ast::Program;
use mvl::mvl::parser::Parser;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn parse(src: &str) -> Program {
    let (mut p, _) = Parser::new(src);
    p.parse_program()
}

fn check(prog: &Program, mode: SolverMode) {
    let _ = check_with_two_preludes_mode(&[], &[], prog, mode);
}

// ── Micro-benchmark programs ──────────────────────────────────────────────────
//
// Each program is crafted to exercise one primary solver layer.
// Real programs will cascade through multiple layers; these isolate each one.

/// Layer 1 — trivial literal: `42` satisfies `self > 0` by constant evaluation.
const L1_LITERAL: &str = r#"
total fn needs_pos(x: Int where x > 0) -> Int { x }
total fn caller() -> Int { needs_pos(42) }
"#;

/// Layer 1 — subsumption: caller carries the same predicate as callee.
const L1_SUBSUME: &str = r#"
total fn needs_pos(x: Int where x > 0) -> Int { x }
total fn passthrough(n: Int where n > 0) -> Int { needs_pos(n) }
"#;

/// Layer 2 — interval arithmetic: `n >= 1 && n <= 100` implies `n > 0`.
const L2_INTERVAL: &str = r#"
total fn needs_pos(x: Int where x > 0) -> Int { x }
total fn clamped(n: Int where n >= 1 && n <= 100) -> Int { needs_pos(n) }
"#;

/// Layer 2 — range: `self >= 0 && self <= 255` refined return.
const L2_RANGE: &str = r#"
type Byte = Int where self >= 0 && self <= 255
total fn clamp(x: Int) -> Byte {
    if x < 0 { 0 } else if x > 255 { 255 } else { x }
}
"#;

/// Layer 3 — symbolic: pure function with two control-flow paths.
const L3_SYMBOLIC: &str = r#"
total fn abs(x: Int) -> Int where self >= 0 {
    if x >= 0 { x } else { 0 - x }
}
"#;

/// Layer 4 — Cooper: linear-arithmetic refinement with multiplication by 2.
const L4_COOPER: &str = r#"
total fn double_pos(x: Int where x > 0) -> Int where self > 0 {
    x + x
}
"#;

/// Layer 5 — Z3 fallback: non-trivial ensures that Cooper cannot close.
const L5_Z3: &str = r#"
total fn clamp_add(x: Int where x >= 0 && x <= 100,
                   y: Int where y >= 0 && y <= 100) -> Int where self >= 0 {
    x + y
}
"#;

// ── Corpus programs ───────────────────────────────────────────────────────────

const CORPUS_FULLY_PROVEN: &str =
    include_str!("../tests/corpus/09_refinements/refinements_fully_proven.mvl");

const CORPUS_REFINEMENTS_VALID: &str =
    include_str!("../tests/corpus/09_refinements/refinements_valid.mvl");

const CORPUS_CONTRACTS: &str = include_str!("../tests/corpus/11_contracts/basic_contracts.mvl");

// ── Benchmark: micro per-layer ────────────────────────────────────────────────

fn bench_layers(c: &mut Criterion) {
    let cases: &[(&str, &str)] = &[
        ("l1_literal", L1_LITERAL),
        ("l1_subsume", L1_SUBSUME),
        ("l2_interval", L2_INTERVAL),
        ("l2_range", L2_RANGE),
        ("l3_symbolic", L3_SYMBOLIC),
        ("l4_cooper", L4_COOPER),
        ("l5_z3", L5_Z3),
    ];

    let mut group = c.benchmark_group("layer");
    for (name, src) in cases {
        let prog = parse(src);
        group.bench_function(*name, |b| b.iter(|| check(&prog, SolverMode::Layered)));
    }
    group.finish();
}

// ── Benchmark: mode comparison ────────────────────────────────────────────────

fn bench_modes(c: &mut Criterion) {
    let modes = [
        SolverMode::Layered,
        SolverMode::FastOnly,
        SolverMode::Z3Only,
    ];

    let cases: &[(&str, &str)] = &[
        ("fully_proven", CORPUS_FULLY_PROVEN),
        ("refinements_valid", CORPUS_REFINEMENTS_VALID),
        ("contracts_basic", CORPUS_CONTRACTS),
    ];

    let mut group = c.benchmark_group("mode");
    for (name, src) in cases {
        let prog = parse(src);
        for mode in modes {
            group.bench_with_input(BenchmarkId::new(*name, mode.as_str()), &mode, |b, &mode| {
                b.iter(|| check(&prog, mode))
            });
        }
    }
    group.finish();
}

// ── Benchmark: corpus full check ──────────────────────────────────────────────

fn bench_corpus(c: &mut Criterion) {
    let sources: &[(&str, &str)] = &[
        ("fully_proven", CORPUS_FULLY_PROVEN),
        ("refinements_valid", CORPUS_REFINEMENTS_VALID),
        ("contracts_basic", CORPUS_CONTRACTS),
    ];

    let mut group = c.benchmark_group("corpus");
    for (name, src) in sources {
        let prog = parse(src);
        group.bench_function(*name, |b| b.iter(|| check(&prog, SolverMode::Layered)));
    }
    group.finish();
}

criterion_group!(benches, bench_layers, bench_modes, bench_corpus);
criterion_main!(benches);
