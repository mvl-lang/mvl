//! Information Flow Control: security lattice operations.
//!
//! Implements Requirement 11 of the MVL spec (003-information-flow).
//!
//! Security lattice (highest to lowest sensitivity):
//!   Secret (3) > Tainted (2) > Clean (1) > Public (0)
//!
//! Upward flow (lower → higher sensitivity) is always allowed.
//! Downward flow requires `declassify()` (Secret→Public) or `sanitize()` (Tainted→Clean).

use crate::mvl::checker::types::Ty;
use crate::mvl::parser::ast::SecurityLabel;

/// Numeric rank for the security lattice (higher = more sensitive).
pub fn lattice_rank(label: SecurityLabel) -> u8 {
    match label {
        SecurityLabel::Public => 0,
        SecurityLabel::Clean => 1,
        SecurityLabel::Tainted => 2,
        SecurityLabel::Secret => 3,
    }
}

/// True if data with label `from` may flow to a context requiring label `to`
/// without explicit declassification or sanitization.
///
/// Upward flow (from lower to higher sensitivity) is always allowed.
pub fn can_flow(from: SecurityLabel, to: SecurityLabel) -> bool {
    lattice_rank(from) <= lattice_rank(to)
}

/// Compute the join (least upper bound) of two labels — the higher-sensitivity one.
pub fn join(a: SecurityLabel, b: SecurityLabel) -> SecurityLabel {
    if lattice_rank(a) >= lattice_rank(b) {
        a
    } else {
        b
    }
}

/// Compute the join of two optional labels.
/// `None` represents an unlabeled type (treated as Public for join purposes).
pub fn join_opt(a: Option<SecurityLabel>, b: Option<SecurityLabel>) -> Option<SecurityLabel> {
    match (a, b) {
        (None, None) => None,
        (Some(l), None) | (None, Some(l)) => Some(l),
        (Some(la), Some(lb)) => Some(join(la, lb)),
    }
}

/// Extract the outermost security label from a type, if any.
/// Looks through Refined wrappers to find the label.
pub fn label_of(ty: &Ty) -> Option<SecurityLabel> {
    match ty {
        Ty::Labeled(l, _) => Some(*l),
        Ty::Refined(inner, _) => label_of(inner),
        _ => None,
    }
}

/// Wrap a type in a security label, or return it unchanged if label is None.
pub fn apply_label(label: Option<SecurityLabel>, ty: Ty) -> Ty {
    match label {
        Some(l) => Ty::Labeled(l, Box::new(ty)),
        None => ty,
    }
}

/// Human-readable name for a security label.
pub fn label_name(label: SecurityLabel) -> &'static str {
    match label {
        SecurityLabel::Public => "Public",
        SecurityLabel::Tainted => "Tainted",
        SecurityLabel::Secret => "Secret",
        SecurityLabel::Clean => "Clean",
    }
}
