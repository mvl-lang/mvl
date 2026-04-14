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
    },

    // ── Effect checking (#19) ────────────────────────────────────────────
    /// Function declares an effect name not in the permitted set (Req 2).
    InvalidEffectName {
        name: String,
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

    // ── Information flow control (#23) ───────────────────────────────────
    /// `declassify()` applied to a non-`Secret<T>` type.
    InvalidDeclassify {
        found: String,
        span: Span,
    },
    /// `sanitize()` applied to a non-`Tainted<T>` type.
    InvalidSanitize {
        found: String,
        span: Span,
    },
    /// `println`/`print` called with a `Secret` or `Tainted` argument.
    ///
    /// Logging functions MUST accept only `Public<T>` per 003-information-flow/Req 6.
    LoggingLabelViolation {
        label: String,
        span: Span,
    },
    /// A `println`/`print` call appears inside a branch controlled by a
    /// `Secret` or `Tainted` condition, creating an implicit information flow.
    ///
    /// Even if the arguments are `Public`, whether the print fires reveals the
    /// secret condition value — a classic implicit (or covert-channel) flow.
    /// Per 003-information-flow: the PC label MUST NOT exceed the label of any
    /// output sink. (Req 11, Phase 3)
    ImplicitFlowViolation {
        /// The label of the controlling condition (e.g. "Secret" or "Tainted").
        pc_label: String,
        /// The name of the public sink (`println` or `print`).
        sink: String,
        span: Span,
    },
    /// `extern` block declares an unsupported ABI.
    UnsupportedExternAbi {
        abi: String,
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
            | CheckError::WrongArgCount { .. }
            | CheckError::MissingField { .. }
            | CheckError::UnknownField { .. }
            | CheckError::FieldNotFound { .. }
            | CheckError::FieldAccessOnEnum { .. }
            | CheckError::UnknownVariant { .. }
            | CheckError::NotAStruct { .. } => 1,
            // Req 2: Memory Safety
            CheckError::UseAfterMove { .. } => 2,
            // Req 3: Totality (exhaustive match)
            CheckError::NonExhaustiveMatch { .. } => 3,
            // Req 4: Null Elimination
            CheckError::OptionDirectAccess { .. } => 4,
            // Req 5: Error Visibility
            CheckError::ResultIgnored { .. }
            | CheckError::PropagateNotResult { .. }
            | CheckError::PropagateIncompatibleError { .. } => 5,
            // Req 6: Ownership (immutability / linearity)
            CheckError::AssignToImmutable { .. }
            | CheckError::MutateImmutableField { .. }
            | CheckError::CaptureMutabilityViolation { .. } => 6,
            // Req 7: Effect Tracking (includes invalid names)
            CheckError::InvalidEffectName { .. }
            | CheckError::UndeclaredEffect { .. }
            | CheckError::MissingEffect { .. } => 7,
            // Req 8: Termination
            CheckError::UnboundedLoopInTotal { .. }
            | CheckError::PartialCallInTotal { .. }
            | CheckError::UnprovenRecursion { .. } => 8,
            // Req 9: Data Race Freedom
            CheckError::CapabilityViolation { .. } | CheckError::IsoAliasingViolation { .. } => 9,
            // Req 10: Refinement Types
            CheckError::RefinementViolated { .. } => 10,
            // Req 11: Information Flow Control
            CheckError::InvalidDeclassify { .. }
            | CheckError::InvalidSanitize { .. }
            | CheckError::LoggingLabelViolation { .. }
            | CheckError::ImplicitFlowViolation { .. } => 11,
            // Req 1: Type Safety (declaration-level — malformed extern ABI is a type/decl error,
            // not an IFC violation; grouping it under Req 11 would pollute IFC metrics).
            CheckError::UnsupportedExternAbi { .. } => 1,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            CheckError::TypeMismatch { span, .. }
            | CheckError::UndefinedVariable { span, .. }
            | CheckError::UndefinedType { span, .. }
            | CheckError::NonNumericArithmetic { span, .. }
            | CheckError::ArithmeticTypeMismatch { span, .. }
            | CheckError::LogicTypeMismatch { span, .. }
            | CheckError::UndefinedFunction { span, .. }
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
            | CheckError::PropagateNotResult { span, .. }
            | CheckError::AssignToImmutable { span, .. }
            | CheckError::MutateImmutableField { span, .. }
            | CheckError::CaptureMutabilityViolation { span, .. }
            | CheckError::UseAfterMove { span, .. }
            | CheckError::RefinementViolated { span, .. }
            | CheckError::InvalidEffectName { span, .. }
            | CheckError::UndeclaredEffect { span, .. }
            | CheckError::MissingEffect { span, .. }
            | CheckError::UnboundedLoopInTotal { span }
            | CheckError::PartialCallInTotal { span, .. }
            | CheckError::UnprovenRecursion { span, .. }
            | CheckError::CapabilityViolation { span, .. }
            | CheckError::IsoAliasingViolation { span, .. }
            | CheckError::InvalidDeclassify { span, .. }
            | CheckError::InvalidSanitize { span, .. }
            | CheckError::LoggingLabelViolation { span, .. }
            | CheckError::ImplicitFlowViolation { span, .. }
            | CheckError::UnsupportedExternAbi { span, .. }
            | CheckError::PropagateIncompatibleError { span, .. } => *span,
        }
    }

    pub fn message(&self) -> String {
        match self {
            CheckError::TypeMismatch {
                expected, found, ..
            } => format!("type mismatch: expected `{expected}`, found `{found}`"),
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
            CheckError::RefinementViolated { pred, .. } => {
                format!("refinement predicate violated: `{pred}`")
            }
            CheckError::InvalidEffectName { name, .. } => format!(
                "unknown effect `{name}` — valid effects are: Console, FileRead, FileWrite, FileDelete, Net, DB, ProcessSpawn, Random, Clock, Env, Log, Async"
            ),
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
            CheckError::InvalidDeclassify { found, .. } => format!(
                "`declassify()` requires `Secret<T>`, found `{found}` — only Secret data can be declassified (for Tainted data use `sanitize()` instead)"
            ),
            CheckError::InvalidSanitize { found, .. } => format!(
                "`sanitize()` requires `Tainted<T>`, found `{found}` — only Tainted data can be sanitized (for Secret data use `declassify()` instead)"
            ),
            CheckError::LoggingLabelViolation { label, .. } => format!(
                "logging functions accept only `Public<T>` but argument has label `{label}` — declassify or sanitize before logging"
            ),
            CheckError::ImplicitFlowViolation { pc_label, sink, .. } => format!(
                "implicit information flow: `{sink}` call inside a branch controlled by a `{pc_label}` condition leaks information via control flow — move the call outside the branch or declassify the condition"
            ),
            CheckError::UnsupportedExternAbi { abi, .. } => format!(
                "unsupported extern ABI `\"{abi}\"` — only \"rust\" and \"c\" are allowed"
            ),
        }
    }
}
