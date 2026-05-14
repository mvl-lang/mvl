//! Requirement verdict tests — one `Proven` and one `Failed` test per requirement.
//!
//! Each test loads a corpus file and runs the corresponding verification pass,
//! asserting the expected verdict.  This file is the canonical demonstration
//! that all 11 checkers are working correctly.
//!
//! Run with: `cargo test --test requirements`
//! Make target: `make test-requirements`
//!
//! Requirements:
//!   1  Type Safety          — BasicCheckPass (Phase 1)
//!   2  Memory Safety        — BasicCheckPass (Phase 3)
//!   3  Totality             — BasicCheckPass (Phase 1)
//!   4  Null Elimination     — BasicCheckPass (Phase 1)
//!   5  Error Visibility     — BasicCheckPass (Phase 1)
//!   6  Ownership            — BasicCheckPass (Phase 1)
//!   7  Effects              — BasicCheckPass (Phase 1)
//!   8  Termination          — BasicCheckPass (Phase 1)
//!   9  Data Race Freedom    — DataRaceFreedomPass (Phase 3)
//!  10  Refinement Types     — RefinementsPass (Phase 3)
//!  11  IFC                  — IFCPass (Phase 3)

use mvl::mvl::checker::check;
use mvl::mvl::checker::passes::{PassRegistry, Verdict};
use mvl::mvl::parser::Parser;

fn run(src: &str, req: u8) -> Verdict {
    let (mut p, _) = Parser::new(src);
    let prog = p.parse_program();
    let result = check(&prog);
    PassRegistry::default_registry().run_req(req, &prog, &result)
}

// ── Req 1: Type Safety ────────────────────────────────────────────────────────

/// Clean typed program → Proven.
#[test]
fn req01_type_safety_proven() {
    let v = run(include_str!("corpus/02_types/basic_types.mvl"), 1);
    assert!(
        v.is_proven(),
        "Req 1 must be Proven on clean types corpus, got: {v:?}"
    );
}

/// Type mismatch → Failed.
#[test]
fn req01_type_safety_failed() {
    let v = run(include_str!("negative/req01/type_mismatch.mvl"), 1);
    assert!(
        v.is_failed(),
        "Req 1 must be Failed on type mismatch corpus, got: {v:?}"
    );
}

// ── Req 2: Memory Safety ──────────────────────────────────────────────────────

/// Clean ownership program → Proven.
#[test]
fn req02_memory_safety_proven() {
    let v = run(include_str!("corpus/04_ownership/ownership.mvl"), 2);
    assert!(
        v.is_proven(),
        "Req 2 must be Proven on clean ownership corpus, got: {v:?}"
    );
}

/// Use-after-move → Failed.
#[test]
fn req02_memory_safety_failed() {
    let v = run(include_str!("negative/req02/use_after_move.mvl"), 2);
    assert!(
        v.is_failed(),
        "Req 2 must be Failed on use-after-move corpus, got: {v:?}"
    );
}

// ── Req 3: Totality ───────────────────────────────────────────────────────────

/// Exhaustive matches, no partial calls → Proven.
#[test]
fn req03_totality_proven() {
    let v = run(include_str!("corpus/02_types/exhaustive_match.mvl"), 3);
    assert!(
        v.is_proven(),
        "Req 3 must be Proven on exhaustive match corpus, got: {v:?}"
    );
}

/// Non-exhaustive match → Failed.
#[test]
fn req03_totality_failed() {
    let v = run(include_str!("negative/req03/missing_arm.mvl"), 3);
    assert!(
        v.is_failed(),
        "Req 3 must be Failed on missing-arm corpus, got: {v:?}"
    );
}

// ── Req 4: Null Elimination ───────────────────────────────────────────────────

/// Proper Option handling → Proven.
#[test]
fn req04_null_elimination_proven() {
    let v = run(include_str!("corpus/02_types/option_result.mvl"), 4);
    assert!(
        v.is_proven(),
        "Req 4 must be Proven on option_result corpus, got: {v:?}"
    );
}

/// Direct field access on Option → Failed.
#[test]
fn req04_null_elimination_failed() {
    let v = run(include_str!("negative/req04/option_field_access.mvl"), 4);
    assert!(
        v.is_failed(),
        "Req 4 must be Failed on option field access corpus, got: {v:?}"
    );
}

// ── Req 5: Error Visibility ───────────────────────────────────────────────────

/// All Results handled → Proven.
#[test]
fn req05_error_visibility_proven() {
    let v = run(include_str!("corpus/02_types/option_result.mvl"), 5);
    assert!(
        v.is_proven(),
        "Req 5 must be Proven on option_result corpus, got: {v:?}"
    );
}

/// Ignored Result → Failed.
#[test]
fn req05_error_visibility_failed() {
    let v = run(include_str!("negative/req05/result_ignored.mvl"), 5);
    assert!(
        v.is_failed(),
        "Req 5 must be Failed on result_ignored corpus, got: {v:?}"
    );
}

// ── Req 6: Ownership ──────────────────────────────────────────────────────────

/// No immutability violations → Proven.
#[test]
fn req06_ownership_proven() {
    let v = run(include_str!("corpus/04_ownership/ownership.mvl"), 6);
    assert!(
        v.is_proven(),
        "Req 6 must be Proven on clean ownership corpus, got: {v:?}"
    );
}

/// Mutation of immutable binding → Failed.
#[test]
fn req06_ownership_failed() {
    let v = run(include_str!("negative/req06/reassign_immutable.mvl"), 6);
    assert!(
        v.is_failed(),
        "Req 6 must be Failed on reassign_immutable corpus, got: {v:?}"
    );
}

// ── Req 7: Effects ────────────────────────────────────────────────────────────

/// All effects declared and propagated → Proven.
#[test]
fn req07_effects_proven() {
    let v = run(include_str!("corpus/05_effects/propagation.mvl"), 7);
    assert!(
        v.is_proven(),
        "Req 7 must be Proven on effects propagation corpus, got: {v:?}"
    );
}

/// Undeclared effect → Failed.
#[test]
fn req07_effects_failed() {
    let v = run(include_str!("negative/req07/undeclared_effect.mvl"), 7);
    assert!(
        v.is_failed(),
        "Req 7 must be Failed on undeclared_effect corpus, got: {v:?}"
    );
}

// ── Req 8: Termination ────────────────────────────────────────────────────────

/// Total functions use only bounded loops → Proven.
#[test]
fn req08_termination_proven() {
    let v = run(
        include_str!("corpus/08_termination/total_vs_partial.mvl"),
        8,
    );
    assert!(
        v.is_proven(),
        "Req 8 must be Proven on total_vs_partial corpus, got: {v:?}"
    );
}

/// While loop in total function → Failed.
#[test]
fn req08_termination_failed() {
    let v = run(include_str!("negative/req08/while_in_total.mvl"), 8);
    assert!(
        v.is_failed(),
        "Req 8 must be Failed on while_in_total corpus, got: {v:?}"
    );
}

// ── Req 9: Data Race Freedom ──────────────────────────────────────────────────

/// All functions use only iso/val capabilities (no ref) → Proven.
#[test]
fn req09_data_race_freedom_proven() {
    let v = run(include_str!("corpus/09_concurrency/race_free_fns.mvl"), 9);
    assert!(
        v.is_proven(),
        "Req 9 must be Proven on race_free_fns corpus, got: {v:?}"
    );
}

/// Actor pub fn behaviors (sendable params only) counted as race-free → Proven (#63/#506).
#[test]
fn req09_data_race_freedom_actors_proven() {
    let v = run(include_str!("corpus/09_concurrency/actors.mvl"), 9);
    assert!(
        v.is_proven(),
        "Req 9 must be Proven on actors corpus, got: {v:?}"
    );
}

/// iso parameter aliased without consume() → Failed.
#[test]
fn req09_data_race_freedom_failed() {
    let v = run(include_str!("negative/req09/iso_aliased.mvl"), 9);
    assert!(
        v.is_failed(),
        "Req 9 must be Failed on iso_aliased corpus, got: {v:?}"
    );
}

// ── Req 10: Refinement Types ──────────────────────────────────────────────────

/// All refined call sites statically proven → Proven.
#[test]
fn req10_refinements_proven() {
    let v = run(
        include_str!("corpus/07_refinements/refinements_fully_proven.mvl"),
        10,
    );
    assert!(
        v.is_proven(),
        "Req 10 must be Proven on fully_proven corpus, got: {v:?}"
    );
}

/// Literal 0 passed to NonZero parameter → Failed.
#[test]
fn req10_refinements_failed() {
    let v = run(include_str!("negative/req10/division_by_zero.mvl"), 10);
    assert!(
        v.is_failed(),
        "Req 10 must be Failed on division_by_zero corpus, got: {v:?}"
    );
}

// ── Req 11: Information Flow Control ─────────────────────────────────────────

/// Security-labeled types, no violations → Proven.
#[test]
fn req11_ifc_proven() {
    let v = run(include_str!("corpus/06_ifc/labels.mvl"), 11);
    assert!(
        v.is_proven(),
        "Req 11 must be Proven on labels corpus, got: {v:?}"
    );
}

/// Tainted condition controls public Console output → Failed.
#[test]
fn req11_ifc_failed() {
    let v = run(include_str!("negative/req11/tainted_to_public.mvl"), 11);
    assert!(
        v.is_failed(),
        "Req 11 must be Failed on tainted_to_public corpus, got: {v:?}"
    );
}
