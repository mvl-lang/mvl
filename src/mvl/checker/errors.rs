// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Type checker error variants.
//!
//! Each variant carries a [`Span`] for precise source location reporting and
//! enough context to produce a human-readable message.

use crate::mvl::parser::lexer::Span;

/// A type-system violation found during checking.
#[derive(Debug, Clone, PartialEq)]
pub enum CheckError {
    // ── Basic type checking (#11) ────────────────────────────────────────
    TypeMismatch {
        expected: String,
        found: String,
        span: Span,
    },
    /// Type mismatch caused by IFC label differences (#1027).
    /// E.g. `Secret[String]` passed where `String` expected, or vice versa.
    /// Mapped to Req 11 (IFC), not Req 1 (Type Safety).
    LabelMismatch {
        expected: String,
        found: String,
        span: Span,
    },
    UndefinedVariable {
        name: String,
        span: Span,
    },
    UndefinedType {
        name: String,
        span: Span,
    },
    NonNumericArithmetic {
        ty: String,
        span: Span,
    },
    ArithmeticTypeMismatch {
        op: String,
        left: String,
        right: String,
        span: Span,
    },
    LogicTypeMismatch {
        op: String,
        ty: String,
        span: Span,
    },
    UndefinedFunction {
        name: String,
        span: Span,
    },
    /// Method not found on a builtin type with a closed method set (#985).
    UnknownMethod {
        receiver_ty: String,
        method: String,
        span: Span,
    },
    WrongArgCount {
        name: String,
        expected: usize,
        found: usize,
        span: Span,
    },

    // ── ADT checking (#12) ───────────────────────────────────────────────
    MissingField {
        ty: String,
        field: String,
        span: Span,
    },
    UnknownField {
        ty: String,
        field: String,
        span: Span,
    },
    FieldNotFound {
        ty: String,
        field: String,
        span: Span,
    },
    FieldAccessOnEnum {
        ty: String,
        span: Span,
    },
    UnknownVariant {
        ty: String,
        variant: String,
        span: Span,
    },
    NotAStruct {
        ty: String,
        span: Span,
    },

    // ── Exhaustive match (#13) ───────────────────────────────────────────
    NonExhaustiveMatch {
        missing: Vec<String>,
        span: Span,
    },

    // ── Option/Result enforcement (#14) ─────────────────────────────────
    OptionDirectAccess {
        span: Span,
    },
    ResultIgnored {
        span: Span,
    },
    OptionIgnored {
        span: Span,
    },
    PropagateNotResult {
        ty: String,
        span: Span,
    },
    PropagateIncompatibleError {
        from_ty: String,
        into_ty: String,
        span: Span,
    },

    // ── Immutability enforcement (#17) ───────────────────────────────────
    AssignToImmutable {
        name: String,
        span: Span,
    },
    MutateImmutableField {
        ty: String,
        field: String,
        span: Span,
    },

    // ── Ownership (#15) ──────────────────────────────────────────────────
    UseAfterMove {
        name: String,
        span: Span,
    },

    // ── Lifetime safety (Phase C, #305) ──────────────────────────────────
    /// A `val`/`ref` reference to a local variable escapes its owner's scope.
    ///
    /// This happens when a function's return type is `val T` or `ref T` but
    /// the referenced value is a local binding that would be deallocated on return.
    ReferenceEscapesScope {
        /// Name of the referenced variable (if known).
        name: String,
        span: Span,
    },
    /// A `val`/`ref` reference is assigned to a binding with a shallower scope
    /// depth than the referent, meaning the reference would outlive the owner.
    ///
    /// Emitted by Phase C scope-depth comparison in `check_stmt` (#305, #363).
    ReferenceOutlivesOwner {
        /// The reference binding being created.
        ref_name: String,
        /// The variable being borrowed.
        owner_name: String,
        span: Span,
    },

    // ── Mutable reference alias checking (Phase D, #306) ─────────────────
    /// A mutable reference `ref x` was requested while `x` is already borrowed
    /// (either shared or mutably).
    AliasingMutableBorrow {
        name: String,
        span: Span,
    },
    /// Two mutable references `ref x` were created for the same variable.
    DoubleMutableBorrow {
        name: String,
        span: Span,
    },

    // ── Lambda capture immutability (ADR-0002) ───────────────────────────
    /// Lambda captures a `mut` binding from an outer scope (ADR-0002).
    CaptureMutabilityViolation {
        name: String,
        span: Span,
    },

    // ── Refinement types (#16) ───────────────────────────────────────────
    RefinementViolated {
        pred: String,
        span: Span,
        /// Counterexample witness value, if the solver extracted one (Phase 4, #627).
        counterexample: Option<String>,
    },

    // ── Effect checking (#19) ────────────────────────────────────────────
    /// Function declares an effect name not in the permitted set (Req 2).
    InvalidEffectName {
        name: String,
        span: Span,
    },
    /// An effect declaration names an unknown parent (#853).
    UnknownEffectParent {
        effect: String,
        parent: String,
        span: Span,
    },
    /// A cycle was detected in the effect subsumption hierarchy (#853).
    EffectCycle {
        /// Chain of effects forming the cycle, e.g. ["A", "B", "A"].
        chain: Vec<String>,
        span: Span,
    },
    /// Pure function body calls an effectful function.
    UndeclaredEffect {
        /// The effectful callee.
        callee: String,
        /// The required effect name.
        effect: String,
        span: Span,
    },

    // ── Effect propagation (#20) ─────────────────────────────────────────
    /// Caller does not declare all effects required by callee.
    MissingEffect {
        caller: String,
        callee: String,
        effect: String,
        span: Span,
    },

    // ── Totality checking (#21) ──────────────────────────────────────────
    /// `while` (unbounded loop) inside a total function.
    UnboundedLoopInTotal {
        span: Span,
    },
    /// Total function calls a `partial` function.
    PartialCallInTotal {
        callee: String,
        span: Span,
    },
    /// Total function is recursive but no argument provably decreases.
    UnprovenRecursion {
        fn_name: String,
        span: Span,
    },

    // ── Reference capability checking (#22) ──────────────────────────────
    /// Value with `ref` (or non-sendable) capability sent across actor boundary.
    CapabilityViolation {
        param: String,
        capability: String,
        span: Span,
    },
    /// `iso` variable bound to a new `let` without `consume()` — would create
    /// two live references to the same isolated object (Req 9, spec 008 §Req 2).
    IsoAliasingViolation {
        name: String,
        span: Span,
    },
    /// `ref` parameter used as a field value in a `spawn` expression — would
    /// give the new actor a reference to a mutably aliased value, creating a
    /// data race between the spawner and the spawned actor (Req 9).
    RefEscapesToConcurrentContext {
        name: String,
        actor_type: String,
        span: Span,
    },
    /// An actor declares two fields with the same name.
    DuplicateActorField {
        actor: String,
        field: String,
        span: Span,
    },
    /// An actor declares two methods with the same name.
    DuplicateActorMethod {
        actor: String,
        method: String,
        span: Span,
    },
    /// A `pub fn` (behavior) on an actor declares a non-`Unit` return type.
    /// Actor behaviors are fire-and-forget; they must return `Unit`.
    NonUnitBehaviorReturn {
        actor: String,
        method: String,
        found: String,
        span: Span,
    },
    /// Linear type (String, List, Map, Set, or named struct) assigned without `consume()`.
    /// MVL uses Pony-style destructive read: ownership transfer requires explicit `consume(x)`.
    LinearTypeBareBind {
        name: String,
        ty: String,
        span: Span,
    },

    // ── Iterator trait (001-type-system/Req 11) ──────────────────────────
    /// Expression after `in` does not implement the `Iterator` trait.
    NotIterator {
        ty: String,
        span: Span,
    },
    // ── Generics constraint enforcement (001-type-system/Req 1) ─────────
    /// Unconstrained type parameter used with an operator that requires a trait bound.
    MissingConstraint {
        type_param: String,
        required_bound: String,
        span: Span,
    },

    // ── Trait impl completeness (#990) ───────────────────────────────────
    /// A trait `impl` block is missing a required method.
    ///
    /// This is caught before transpilation so the backend never emits `todo!()`
    /// stubs for unfilled trait methods (#990).
    MissingTraitMethod {
        /// The trait being implemented (e.g. `"Display"`).
        trait_name: String,
        /// The type implementing the trait (e.g. `"Point"`).
        type_name: String,
        /// The method name that must be provided (e.g. `"fmt"`).
        method: String,
        span: Span,
    },

    // ── Information flow control (#23, #894) ─────────────────────────────
    /// `relabel name(expr, "tag")` applied to wrong input type (#894).
    InvalidRelabel {
        /// Name of the relabel transition (e.g. `"trust"`).
        transition: String,
        /// The label name the transition expects on input (or `"bare"`).
        expected_from: String,
        /// The actual type of the argument.
        found: String,
        span: Span,
    },
    /// `relabel name(...)` but `name` is not a declared relabel transition (#894).
    UnknownRelabel {
        name: String,
        span: Span,
    },
    // Kept for error-code stability; callers that matched InvalidDeclassify/InvalidSanitize
    // should migrate to InvalidRelabel.
    #[allow(dead_code)]
    InvalidDeclassify {
        found: String,
        span: Span,
    },
    #[allow(dead_code)]
    InvalidSanitize {
        found: String,
        span: Span,
    },
    /// An effectful (observable) function call appears inside a branch controlled by a
    /// `Secret` or `Tainted` condition, creating an implicit information flow.
    ///
    /// Even if the arguments are bare, whether the call fires reveals the
    /// secret condition value — a classic implicit (or covert-channel) flow.
    /// Per 003-information-flow: the PC label MUST NOT reach any observable function.
    /// (Req 11, #1007)
    ImplicitFlowViolation {
        /// The label of the controlling condition (e.g. "Secret" or "Tainted").
        pc_label: String,
        /// The name of the observable (effectful) function.
        observable_fn: String,
        span: Span,
    },
    /// A function is called inside a branch controlled by a labeled condition, and that
    /// function transitively reaches an observable (effectful) function — cross-function implicit flow.
    ///
    /// Example: `if secret { log_access("x") }` where `log_access` calls `println`.
    /// Whether `log_access` fires reveals the secret condition value.
    /// (Req 11, #1007 cross-function)
    CrossFunctionImplicitFlowViolation {
        /// The label of the controlling condition.
        pc_label: String,
        /// The function in which this call-under-high-PC occurs.
        caller: String,
        /// The function called under high PC.
        callee: String,
        /// The observable (effectful) function reachable from `callee` (directly or transitively).
        observable_fn: String,
        span: Span,
    },
    /// `extern` block declares an unsupported ABI.
    UnsupportedExternAbi {
        abi: String,
        span: Span,
    },

    // ── Function contracts (#621) ─────────────────────────────────────────
    /// A `requires` precondition was statically proven to be violated at this call site.
    PreconditionViolated {
        fn_name: String,
        pred: String,
        span: Span,
        /// Counterexample witness value, if the solver extracted one (Phase 4, #627).
        counterexample: Option<String>,
    },
    /// An `ensures` postcondition was statically proven to be violated at this return point.
    PostconditionViolated {
        fn_name: String,
        pred: String,
        span: Span,
        /// Counterexample witness value, if the solver extracted one (Phase 4, #627).
        counterexample: Option<String>,
    },
    /// A `while` loop invariant was statically proven to not hold at loop entry.
    InvariantViolated {
        fn_name: String,
        pred: String,
        span: Span,
        /// Counterexample witness value, if the solver extracted one (Phase 4, #627).
        counterexample: Option<String>,
    },
    /// A `while` loop invariant was statically proven to not be preserved across iterations.
    InvariantNotPreserved {
        fn_name: String,
        pred: String,
        span: Span,
    },
    /// A `decreases` measure is not bounded below (must be ≥ 0) at loop entry.
    DecreasesNotBounded {
        fn_name: String,
        measure: String,
        span: Span,
    },
    /// A `decreases` measure is not proven to strictly decrease across loop iterations.
    DecreasesNotDecreasing {
        fn_name: String,
        measure: String,
        span: Span,
    },
    /// A `forall` or `exists` quantifier was used outside a ghost/contract context.
    QuantifierOutsideGhost {
        span: Span,
    },

    // ── Session type errors (Phase 8, #260) ──────────────────────────────
    /// A session type protocol is violated: the operation performed does not
    /// match what the declared protocol expects at this point.
    SessionProtocolMismatch {
        /// The session type operation that was expected.
        expected: String,
        /// What was actually found / attempted.
        found: String,
        span: Span,
    },
    /// Both sides of a channel are annotated with session types that are not
    /// duals of each other. Both must be declared for the check to fire.
    SessionDualityMismatch {
        /// The protocol declared for one side.
        side_a: String,
        /// The protocol declared for the other side.
        side_b: String,
        span: Span,
    },
    /// A session type choice branch label is not part of the declared protocol.
    SessionUnknownBranch {
        label: String,
        available: Vec<String>,
        span: Span,
    },
    /// A session-typed channel was used after the protocol reached `end`.
    SessionAfterEnd {
        span: Span,
    },
    /// Both sides of a declared dual pair are simultaneously in `Receive` state.
    ///
    /// Neither side will send first, so neither can make progress: deadlock.
    SessionDeadlock {
        span: Span,
    },
    /// A session type choice block contains duplicate branch labels.
    ///
    /// Duplicate labels produce unreachable states because only the first
    /// matching label is ever selected.
    SessionDuplicateLabel {
        label: String,
        span: Span,
    },

    /// Interprocedural IFC violation (#831): inferred arg label cannot flow to
    /// the required parameter label through a chain of unannotated functions.
    ///
    /// The direct type checker missed this because the arg's declared type is
    /// unlabeled (treated as Public), but forward label propagation (#830/#833)
    /// reveals the actual inferred label is higher.
    InterprocFlowViolation {
        /// Name of the function being called.
        callee: String,
        /// Zero-based index of the offending parameter.
        param_idx: usize,
        /// Label required by the parameter (e.g. "Clean").
        required_label: String,
        /// Label inferred for the argument (e.g. "Tainted").
        inferred_label: String,
        /// Simplified call chain tracing the labeled data to its source.
        chain: Vec<String>,
        /// Name of the function containing this call site.
        caller: String,
        /// Source location of the call expression.
        span: Span,
    },
}

impl CheckError {
    /// Returns the MVL requirement number (1–11) this error violates.
    pub fn requirement_number(&self) -> u8 {
        match self {
            // Req 1: Type Safety
            CheckError::TypeMismatch { .. }
            | CheckError::UndefinedVariable { .. }
            | CheckError::UndefinedType { .. }
            | CheckError::NonNumericArithmetic { .. }
            | CheckError::ArithmeticTypeMismatch { .. }
            | CheckError::LogicTypeMismatch { .. }
            | CheckError::UndefinedFunction { .. }
            | CheckError::UnknownMethod { .. }
            | CheckError::WrongArgCount { .. }
            | CheckError::MissingField { .. }
            | CheckError::UnknownField { .. }
            | CheckError::FieldNotFound { .. }
            | CheckError::FieldAccessOnEnum { .. }
            | CheckError::UnknownVariant { .. }
            | CheckError::NotAStruct { .. } => 1,
            // Req 2: Memory Safety
            CheckError::UseAfterMove { .. }
            | CheckError::ReferenceEscapesScope { .. }
            | CheckError::ReferenceOutlivesOwner { .. }
            | CheckError::AliasingMutableBorrow { .. }
            | CheckError::DoubleMutableBorrow { .. } => 2,
            // Req 3: Totality (exhaustive match)
            CheckError::NonExhaustiveMatch { .. } => 3,
            // Req 4: Null Elimination
            CheckError::OptionDirectAccess { .. } => 4,
            // Req 5: Error Visibility
            CheckError::ResultIgnored { .. }
            | CheckError::OptionIgnored { .. }
            | CheckError::PropagateNotResult { .. }
            | CheckError::PropagateIncompatibleError { .. } => 5,
            // Req 6: Ownership (immutability / linearity)
            CheckError::AssignToImmutable { .. }
            | CheckError::MutateImmutableField { .. }
            | CheckError::CaptureMutabilityViolation { .. }
            | CheckError::LinearTypeBareBind { .. } => 6,
            // Req 7: Effect Tracking (includes invalid names)
            CheckError::InvalidEffectName { .. }
            | CheckError::UnknownEffectParent { .. }
            | CheckError::EffectCycle { .. }
            | CheckError::UndeclaredEffect { .. }
            | CheckError::MissingEffect { .. } => 7,
            // Req 8: Termination
            CheckError::UnboundedLoopInTotal { .. }
            | CheckError::PartialCallInTotal { .. }
            | CheckError::UnprovenRecursion { .. }
            | CheckError::DecreasesNotBounded { .. }
            | CheckError::DecreasesNotDecreasing { .. } => 8,
            // Req 9: Data Race Freedom
            CheckError::CapabilityViolation { .. }
            | CheckError::IsoAliasingViolation { .. }
            | CheckError::RefEscapesToConcurrentContext { .. }
            | CheckError::DuplicateActorField { .. }
            | CheckError::DuplicateActorMethod { .. }
            | CheckError::NonUnitBehaviorReturn { .. } => 9,
            // Req 10: Refinement Types & Contracts
            CheckError::RefinementViolated { .. }
            | CheckError::PreconditionViolated { .. }
            | CheckError::PostconditionViolated { .. }
            | CheckError::InvariantViolated { .. }
            | CheckError::InvariantNotPreserved { .. }
            | CheckError::QuantifierOutsideGhost { .. } => 10,
            // Req 1: Type Safety — session type protocol errors
            CheckError::SessionProtocolMismatch { .. }
            | CheckError::SessionDualityMismatch { .. }
            | CheckError::SessionUnknownBranch { .. }
            | CheckError::SessionAfterEnd { .. }
            | CheckError::SessionDeadlock { .. }
            | CheckError::SessionDuplicateLabel { .. } => 1,
            // Req 11: Information Flow Control
            CheckError::InvalidRelabel { .. }
            | CheckError::UnknownRelabel { .. }
            | CheckError::InvalidDeclassify { .. }
            | CheckError::InvalidSanitize { .. }
            | CheckError::ImplicitFlowViolation { .. }
            | CheckError::CrossFunctionImplicitFlowViolation { .. }
            | CheckError::InterprocFlowViolation { .. }
            | CheckError::LabelMismatch { .. } => 11,
            // Req 1: Type Safety (declaration-level — malformed extern ABI is a type/decl error,
            // not an IFC violation; grouping it under Req 11 would pollute IFC metrics).
            CheckError::UnsupportedExternAbi { .. } => 1,
            // Req 1: Type Safety — Iterator trait constraint
            CheckError::NotIterator { .. } => 1,
            // Req 1: Type Safety — generic constraint enforcement (#1028)
            CheckError::MissingConstraint { .. } => 1,
            // Req 1: Type Safety — trait impl completeness (#990)
            CheckError::MissingTraitMethod { .. } => 1,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            CheckError::TypeMismatch { span, .. }
            | CheckError::LabelMismatch { span, .. }
            | CheckError::UndefinedVariable { span, .. }
            | CheckError::UndefinedType { span, .. }
            | CheckError::NonNumericArithmetic { span, .. }
            | CheckError::ArithmeticTypeMismatch { span, .. }
            | CheckError::LogicTypeMismatch { span, .. }
            | CheckError::UndefinedFunction { span, .. }
            | CheckError::UnknownMethod { span, .. }
            | CheckError::WrongArgCount { span, .. }
            | CheckError::MissingField { span, .. }
            | CheckError::UnknownField { span, .. }
            | CheckError::FieldNotFound { span, .. }
            | CheckError::FieldAccessOnEnum { span, .. }
            | CheckError::UnknownVariant { span, .. }
            | CheckError::NotAStruct { span, .. }
            | CheckError::NonExhaustiveMatch { span, .. }
            | CheckError::OptionDirectAccess { span }
            | CheckError::ResultIgnored { span }
            | CheckError::OptionIgnored { span }
            | CheckError::PropagateNotResult { span, .. }
            | CheckError::AssignToImmutable { span, .. }
            | CheckError::MutateImmutableField { span, .. }
            | CheckError::CaptureMutabilityViolation { span, .. }
            | CheckError::UseAfterMove { span, .. }
            | CheckError::ReferenceEscapesScope { span, .. }
            | CheckError::ReferenceOutlivesOwner { span, .. }
            | CheckError::AliasingMutableBorrow { span, .. }
            | CheckError::DoubleMutableBorrow { span, .. }
            | CheckError::RefinementViolated { span, .. }
            | CheckError::InvalidEffectName { span, .. }
            | CheckError::UnknownEffectParent { span, .. }
            | CheckError::EffectCycle { span, .. }
            | CheckError::UndeclaredEffect { span, .. }
            | CheckError::MissingEffect { span, .. }
            | CheckError::UnboundedLoopInTotal { span }
            | CheckError::PartialCallInTotal { span, .. }
            | CheckError::UnprovenRecursion { span, .. }
            | CheckError::CapabilityViolation { span, .. }
            | CheckError::IsoAliasingViolation { span, .. }
            | CheckError::RefEscapesToConcurrentContext { span, .. }
            | CheckError::DuplicateActorField { span, .. }
            | CheckError::DuplicateActorMethod { span, .. }
            | CheckError::NonUnitBehaviorReturn { span, .. }
            | CheckError::LinearTypeBareBind { span, .. }
            | CheckError::InvalidRelabel { span, .. }
            | CheckError::UnknownRelabel { span, .. }
            | CheckError::InvalidDeclassify { span, .. }
            | CheckError::InvalidSanitize { span, .. }
            | CheckError::ImplicitFlowViolation { span, .. }
            | CheckError::CrossFunctionImplicitFlowViolation { span, .. }
            | CheckError::UnsupportedExternAbi { span, .. }
            | CheckError::PropagateIncompatibleError { span, .. }
            | CheckError::NotIterator { span, .. }
            | CheckError::MissingConstraint { span, .. }
            | CheckError::PreconditionViolated { span, .. }
            | CheckError::PostconditionViolated { span, .. }
            | CheckError::InvariantViolated { span, .. }
            | CheckError::InvariantNotPreserved { span, .. }
            | CheckError::DecreasesNotBounded { span, .. }
            | CheckError::DecreasesNotDecreasing { span, .. }
            | CheckError::QuantifierOutsideGhost { span }
            | CheckError::SessionProtocolMismatch { span, .. }
            | CheckError::SessionDualityMismatch { span, .. }
            | CheckError::SessionUnknownBranch { span, .. }
            | CheckError::SessionAfterEnd { span }
            | CheckError::SessionDeadlock { span }
            | CheckError::SessionDuplicateLabel { span, .. }
            | CheckError::InterprocFlowViolation { span, .. }
            | CheckError::MissingTraitMethod { span, .. } => *span,
        }
    }

    pub fn message(&self) -> String {
        match self {
            CheckError::TypeMismatch {
                expected, found, ..
            } => format!("type mismatch: expected `{expected}`, found `{found}`"),
            CheckError::LabelMismatch {
                expected, found, ..
            } => format!("IFC label mismatch: expected `{expected}`, found `{found}` — use `relabel` to convert between labeled and bare types"),
            CheckError::UndefinedVariable { name, .. } => format!("undefined variable `{name}`"),
            CheckError::UndefinedType { name, .. } => format!("undefined type `{name}`"),
            CheckError::NonNumericArithmetic { ty, .. } => {
                format!("arithmetic operation requires numeric type, found `{ty}`")
            }
            CheckError::ArithmeticTypeMismatch {
                op, left, right, ..
            } => format!("type mismatch in `{op}`: `{left}` vs `{right}`"),
            CheckError::LogicTypeMismatch { op, ty, .. } => {
                format!("logical operator `{op}` requires `Bool`, found `{ty}`")
            }
            CheckError::UndefinedFunction { name, .. } => format!("undefined function `{name}`"),
            CheckError::UnknownMethod {
                receiver_ty,
                method,
                ..
            } => format!("no method `{method}` on type `{receiver_ty}`"),
            CheckError::WrongArgCount {
                name,
                expected,
                found,
                ..
            } => format!("function `{name}` expects {expected} argument(s), got {found}"),
            CheckError::MissingField { ty, field, .. } => {
                format!("missing field `{field}` in construction of `{ty}`")
            }
            CheckError::UnknownField { ty, field, .. } => {
                format!("unknown field `{field}` in construction of `{ty}`")
            }
            CheckError::FieldNotFound { ty, field, .. } => {
                format!("no field `{field}` on type `{ty}`")
            }
            CheckError::FieldAccessOnEnum { ty, .. } => {
                format!("cannot access field directly on enum `{ty}` — use `match`")
            }
            CheckError::UnknownVariant { ty, variant, .. } => {
                format!("no variant `{variant}` on enum `{ty}`")
            }
            CheckError::NotAStruct { ty, .. } => format!("`{ty}` is not a struct type"),
            CheckError::NonExhaustiveMatch { missing, .. } => {
                format!("non-exhaustive `match`: missing {}", missing.join(", "))
            }
            CheckError::OptionDirectAccess { .. } => {
                "cannot access `Option<T>` value directly — use `match` or `?`".to_string()
            }
            CheckError::ResultIgnored { .. } => {
                "`Result` value must be used — handle with `match` or propagate with `?`"
                    .to_string()
            }
            CheckError::OptionIgnored { .. } => {
                "`Option` value must be used — handle with `match`, `if let`, or `unwrap_or`"
                    .to_string()
            }
            CheckError::PropagateNotResult { ty, .. } => {
                format!("`?` applied to `{ty}`, which is neither `Result` nor `Option`")
            }
            CheckError::PropagateIncompatibleError { from_ty, into_ty, .. } => {
                format!(
                    "`?` cannot convert error `{from_ty}` into `{into_ty}` — implement `From<{from_ty}> for {into_ty}`"
                )
            }
            CheckError::CaptureMutabilityViolation { name, .. } => format!(
                "lambda captures mutable binding `{name}` — lambdas must have immutable captures only (ADR-0002)"
            ),
            CheckError::AssignToImmutable { name, .. } => {
                format!("cannot assign to immutable binding `{name}`")
            }
            CheckError::MutateImmutableField { ty, field, .. } => {
                format!("cannot assign to immutable field `{field}` on `{ty}`")
            }
            CheckError::UseAfterMove { name, .. } => format!("use of moved value `{name}`"),
            CheckError::ReferenceEscapesScope { name, .. } => format!(
                "reference to `{name}` escapes its scope — the referenced value would be deallocated before the reference is used"
            ),
            CheckError::ReferenceOutlivesOwner {
                ref_name,
                owner_name,
                ..
            } => format!(
                "binding `{ref_name}` of reference type outlives `{owner_name}` — the reference would be dangling when `{owner_name}` is dropped"
            ),
            CheckError::AliasingMutableBorrow { name, .. } => format!(
                "cannot create `ref` to `{name}`: it is already borrowed — release existing references before creating a mutable reference"
            ),
            CheckError::DoubleMutableBorrow { name, .. } => format!(
                "cannot create `ref` to `{name}` more than once at a time — only one mutable reference is allowed"
            ),
            CheckError::RefinementViolated { pred, counterexample, .. } => {
                let cx = counterexample.as_deref().map(|c| format!(" (counterexample: {c})")).unwrap_or_default();
                format!("refinement predicate violated: `{pred}`{cx}")
            }
            CheckError::InvalidEffectName { name, .. } => {
                format!("unknown effect `{name}` — valid effects are declared in std/effects.mvl")
            }
            CheckError::UnknownEffectParent { effect, parent, .. } => format!(
                "effect `{effect}` subsumes unknown effect `{parent}` — declare `{parent}` before using it as a parent"
            ),
            CheckError::EffectCycle { chain, .. } => {
                let path = chain.join(" > ");
                format!("effect subsumption cycle detected: {path}")
            }
            CheckError::UndeclaredEffect { callee, effect, .. } => {
                format!(
                    "function has no effect declaration but calls `{callee}` which requires `! {effect}`"
                )
            }
            CheckError::MissingEffect {
                caller,
                callee,
                effect,
                ..
            } => format!(
                "function `{caller}` calls `{callee}` which requires `! {effect}` but `{caller}` does not declare it"
            ),
            CheckError::UnboundedLoopInTotal { .. } => {
                "unbounded loop in total function — declare function as `partial` to allow non-termination"
                    .to_string()
            }
            CheckError::PartialCallInTotal { callee, .. } => {
                format!(
                    "total function calls `partial` function `{callee}` — total functions cannot call partial ones"
                )
            }
            CheckError::UnprovenRecursion { fn_name, .. } => format!(
                "recursive call in total function `{fn_name}` cannot be proven terminating — argument does not structurally decrease"
            ),
            CheckError::CapabilityViolation {
                param, capability, ..
            } => format!(
                "`{capability}` capability of `{param}` cannot be sent across actor boundary; use `iso` or `val`"
            ),
            CheckError::IsoAliasingViolation { name, .. } => format!(
                "`iso` value `{name}` aliased without `consume()` — use `consume({name})` to transfer ownership and preserve isolation"
            ),
            CheckError::RefEscapesToConcurrentContext {
                name, actor_type, ..
            } => format!(
                "`ref` value `{name}` escapes to concurrent actor `{actor_type}` — use `iso` or `val` for actor field initialization"
            ),
            CheckError::DuplicateActorField { actor, field, .. } => format!(
                "actor `{actor}` declares duplicate field `{field}`"
            ),
            CheckError::DuplicateActorMethod { actor, method, .. } => format!(
                "actor `{actor}` declares duplicate method `{method}`"
            ),
            CheckError::NonUnitBehaviorReturn { actor, method, found, .. } => format!(
                "actor `{actor}` behavior `{method}` must return `Unit` (fire-and-forget), found `{found}`"
            ),
            CheckError::LinearTypeBareBind { name, ty, .. } => format!(
                "bare assignment of linear type `{ty}` — use `consume({name})` to transfer ownership (Pony destructive read semantics)"
            ),
            CheckError::InvalidRelabel { transition, expected_from, found, .. } => format!(
                "`relabel {transition}` expects `{expected_from}[T]` input, found `{found}` — check the declared transition type"
            ),
            CheckError::UnknownRelabel { name, .. } => format!(
                "`relabel {name}` is not declared — add `relabel {name}: From -> To` to introduce it"
            ),
            CheckError::InvalidDeclassify { found, .. } => format!(
                "`declassify()` is removed — use `relabel release({found}, \"TAG\")` instead"
            ),
            CheckError::InvalidSanitize { found, .. } => format!(
                "`sanitize()` is removed — use `relabel trust({found}, \"TAG\")` instead"
            ),
            CheckError::ImplicitFlowViolation { pc_label, observable_fn, .. } => format!(
                "implicit information flow: `{observable_fn}` call inside a branch controlled by a `{pc_label}` condition leaks information via control flow — move the call outside the branch or relabel the condition"
            ),
            CheckError::CrossFunctionImplicitFlowViolation { pc_label, caller, callee, observable_fn, .. } => format!(
                "cross-function implicit flow: `{callee}` called in `{caller}` inside a branch controlled by a `{pc_label}` condition reaches observable function `{observable_fn}` — move the call outside the branch or relabel the condition"
            ),
            CheckError::UnsupportedExternAbi { abi, .. } => format!(
                "unsupported extern ABI `\"{abi}\"` — only \"rust\" and \"c\" are allowed"
            ),
            CheckError::NotIterator { ty, .. } => format!(
                "`{ty}` does not implement `Iterator` — only types with `impl Iterator<T>` can be used in `for...in`"
            ),
            CheckError::MissingConstraint {
                type_param,
                required_bound,
                ..
            } => format!(
                "type parameter `{type_param}` does not implement `{required_bound}` — add `where {type_param}: {required_bound}` to the function signature"
            ),
            CheckError::PreconditionViolated { fn_name, pred, counterexample, .. } => {
                let cx = counterexample.as_deref().map(|c| format!(" (counterexample: {c})")).unwrap_or_default();
                format!("precondition violated for `{fn_name}`: `{pred}` cannot be proven at this call site{cx}")
            }
            CheckError::PostconditionViolated { fn_name, pred, counterexample, .. } => {
                let cx = counterexample.as_deref().map(|c| format!(" (counterexample: {c})")).unwrap_or_default();
                format!("postcondition violated in `{fn_name}`: `{pred}` cannot be proven at this return point{cx}")
            }
            CheckError::InvariantViolated { fn_name, pred, counterexample, .. } => {
                let cx = counterexample.as_deref().map(|c| format!(" (counterexample: {c})")).unwrap_or_default();
                format!("loop invariant `{pred}` in `{fn_name}` cannot be proven to hold at loop entry{cx}")
            }
            CheckError::InvariantNotPreserved { fn_name, pred, .. } => {
                format!("loop invariant `{pred}` in `{fn_name}` is not preserved across loop iterations")
            }
            CheckError::DecreasesNotBounded { fn_name, measure, .. } => {
                format!("`decreases {measure}` in `{fn_name}` cannot be proven to be bounded below (must be ≥ 0) at loop entry")
            }
            CheckError::DecreasesNotDecreasing { fn_name, measure, .. } => {
                format!("`decreases {measure}` in `{fn_name}` cannot be proven to strictly decrease across loop iterations")
            }
            CheckError::QuantifierOutsideGhost { .. } => {
                "`forall`/`exists` quantifiers are only valid inside ghost bindings, `requires`, `ensures`, or `invariant` predicates".to_string()
            }
            CheckError::SessionProtocolMismatch { expected, found, .. } => {
                format!("session protocol mismatch: expected `{expected}`, found `{found}`")
            }
            CheckError::SessionDualityMismatch { side_a, side_b, .. } => {
                format!(
                    "session type duality violation: `{side_a}` and `{side_b}` are not duals of each other"
                )
            }
            CheckError::SessionUnknownBranch { label, available, .. } => {
                let avail = available.join(", ");
                format!(
                    "unknown session branch `{label}`; available branches are: {avail}"
                )
            }
            CheckError::SessionAfterEnd { .. } => {
                "session protocol already reached `end`; no further operations are allowed".to_string()
            }
            CheckError::SessionDeadlock { .. } => {
                "session protocol deadlock: both sides are simultaneously in `receive` state (mutual blocking)".to_string()
            }
            CheckError::SessionDuplicateLabel { label, .. } => {
                format!("session protocol unreachable state: duplicate branch label `{label}`")
            }
            CheckError::InterprocFlowViolation {
                callee,
                param_idx,
                required_label,
                inferred_label,
                chain,
                caller,
                ..
            } => {
                let chain_str = if chain.is_empty() {
                    String::new()
                } else {
                    format!(" (via {})", chain.join(" \u{2192} "))
                };
                format!(
                    "interprocedural IFC: {inferred_label} data from `{caller}` reaches \
                     `{callee}` parameter {param_idx} requiring {required_label}; \
                     use sanitize() or declassify(){chain_str}"
                )
            }
            CheckError::MissingTraitMethod { trait_name, type_name, method, .. } => format!(
                "`impl {trait_name} for {type_name}` is missing required method `{method}` — add a `fn {method}` body to the impl block"
            ),
        }
    }
}
