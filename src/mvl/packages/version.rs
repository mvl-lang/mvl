// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Semver parsing and version constraint checking.
//!
//! Handles `>=1.0.0`, `<2.0.0`, `>=1.0.0, <2.0.0`, `^1.2.3`, `~1.2.3`, and exact version strings.

/// A parsed semantic version triple.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// Parse a strict semver string (`MAJOR.MINOR.PATCH`).
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.trim().split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        let major = parts[0].parse::<u32>().ok()?;
        let minor = parts[1].parse::<u32>().ok()?;
        let patch = parts[2].parse::<u32>().ok()?;
        Some(Version {
            major,
            minor,
            patch,
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// A single version predicate: `>=1.0.0`, `>1.0.0`, `<=2.0.0`, `<2.0.0`, `=1.2.3`.
#[derive(Debug, Clone)]
enum Predicate {
    Gte(Version),
    Gt(Version),
    Lte(Version),
    Lt(Version),
    Eq(Version),
}

impl Predicate {
    fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if let Some(rest) = s.strip_prefix(">=") {
            Some(Predicate::Gte(Version::parse(rest.trim())?))
        } else if let Some(rest) = s.strip_prefix("<=") {
            Some(Predicate::Lte(Version::parse(rest.trim())?))
        } else if let Some(rest) = s.strip_prefix('>') {
            Some(Predicate::Gt(Version::parse(rest.trim())?))
        } else if let Some(rest) = s.strip_prefix('<') {
            Some(Predicate::Lt(Version::parse(rest.trim())?))
        } else if let Some(rest) = s.strip_prefix('=') {
            Some(Predicate::Eq(Version::parse(rest.trim())?))
        } else {
            // Bare version means exact match
            Some(Predicate::Eq(Version::parse(s)?))
        }
    }

    fn matches(&self, v: &Version) -> bool {
        match self {
            Predicate::Gte(min) => v >= min,
            Predicate::Gt(min) => v > min,
            Predicate::Lte(max) => v <= max,
            Predicate::Lt(max) => v < max,
            Predicate::Eq(exact) => v == exact,
        }
    }
}

/// A version constraint: one or more comma-separated predicates that must all match.
#[derive(Debug, Clone)]
pub struct VersionConstraint {
    predicates: Vec<Predicate>,
}

impl VersionConstraint {
    /// Parse a constraint string like `">=1.0.0, <2.0.0"`, `"^1.2.3"`, `"~1.2.3"`, or `"1.2.3"`.
    ///
    /// `^X.Y.Z` expands to `>=X.Y.Z, <(X+1).0.0` (or `<0.(Y+1).0` when X=0, etc.).
    /// `~X.Y.Z` expands to `>=X.Y.Z, <X.(Y+1).0`.
    pub fn parse(s: &str) -> Option<Self> {
        let mut predicates: Vec<Predicate> = Vec::new();
        for part in s.split(',') {
            let part = part.trim();
            if let Some(rest) = part.strip_prefix('^') {
                let v = Version::parse(rest.trim())?;
                // ^X.Y.Z: allow changes that do not modify the left-most non-zero digit
                let upper = if v.major > 0 {
                    Version {
                        major: v.major + 1,
                        minor: 0,
                        patch: 0,
                    }
                } else if v.minor > 0 {
                    Version {
                        major: 0,
                        minor: v.minor + 1,
                        patch: 0,
                    }
                } else {
                    Version {
                        major: 0,
                        minor: 0,
                        patch: v.patch + 1,
                    }
                };
                predicates.push(Predicate::Gte(v));
                predicates.push(Predicate::Lt(upper));
            } else if let Some(rest) = part.strip_prefix('~') {
                let v = Version::parse(rest.trim())?;
                // ~X.Y.Z: allow patch-level changes only
                let upper = Version {
                    major: v.major,
                    minor: v.minor + 1,
                    patch: 0,
                };
                predicates.push(Predicate::Gte(v));
                predicates.push(Predicate::Lt(upper));
            } else {
                predicates.push(Predicate::parse(part)?);
            }
        }
        Some(VersionConstraint { predicates })
    }

    /// Returns true if `version` satisfies all predicates.
    pub fn matches(&self, version: &Version) -> bool {
        self.predicates.iter().all(|p| p.matches(version))
    }
}

/// Given a list of available versions and a constraint, return the highest
/// matching version using Go's Minimum Version Selection principle
/// (select the *minimum required* version that satisfies all constraints).
///
/// For a single constraint this returns the lowest satisfying version.
/// Use `select_maximum` when you want the highest satisfying version.
pub fn select_minimum<'a>(
    available: &'a [Version],
    constraint: &VersionConstraint,
) -> Option<&'a Version> {
    available.iter().filter(|v| constraint.matches(v)).min()
}

/// Return the highest available version satisfying the constraint.
pub fn select_maximum<'a>(
    available: &'a [Version],
    constraint: &VersionConstraint,
) -> Option<&'a Version> {
    available.iter().filter(|v| constraint.matches(v)).max()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Existing tests ────────────────────────────────────────────────────────

    #[test]
    fn parse_valid_semver() {
        let v = Version::parse("1.2.3").unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
    }

    #[test]
    fn parse_invalid_semver() {
        assert!(Version::parse("1.2").is_none());
        assert!(Version::parse("1.2.3.4").is_none());
        assert!(Version::parse("not.a.version").is_none());
        assert!(Version::parse("").is_none());
    }

    #[test]
    fn version_ordering() {
        let v100 = Version::parse("1.0.0").unwrap();
        let v120 = Version::parse("1.2.0").unwrap();
        let v200 = Version::parse("2.0.0").unwrap();
        assert!(v100 < v120);
        assert!(v120 < v200);
        assert_eq!(v100, Version::parse("1.0.0").unwrap());
    }

    #[test]
    fn constraint_gte() {
        let c = VersionConstraint::parse(">=1.0.0").unwrap();
        assert!(c.matches(&Version::parse("1.0.0").unwrap()));
        assert!(c.matches(&Version::parse("1.2.0").unwrap()));
        assert!(!c.matches(&Version::parse("0.9.0").unwrap()));
    }

    #[test]
    fn constraint_range() {
        let c = VersionConstraint::parse(">=1.0.0, <2.0.0").unwrap();
        assert!(c.matches(&Version::parse("1.0.0").unwrap()));
        assert!(c.matches(&Version::parse("1.9.9").unwrap()));
        assert!(!c.matches(&Version::parse("2.0.0").unwrap()));
        assert!(!c.matches(&Version::parse("0.9.0").unwrap()));
    }

    #[test]
    fn constraint_exact() {
        let c = VersionConstraint::parse("1.2.0").unwrap();
        assert!(c.matches(&Version::parse("1.2.0").unwrap()));
        assert!(!c.matches(&Version::parse("1.2.1").unwrap()));
    }

    #[test]
    fn select_minimum_picks_lowest_satisfying() {
        let available: Vec<Version> = ["1.0.0", "1.2.0", "1.5.0", "2.0.0"]
            .iter()
            .map(|s| Version::parse(s).unwrap())
            .collect();
        let c = VersionConstraint::parse(">=1.2.0, <2.0.0").unwrap();
        let selected = select_minimum(&available, &c).unwrap();
        assert_eq!(selected.to_string(), "1.2.0");
    }

    #[test]
    fn select_maximum_picks_highest_satisfying() {
        let available: Vec<Version> = ["1.0.0", "1.2.0", "1.5.0", "2.0.0"]
            .iter()
            .map(|s| Version::parse(s).unwrap())
            .collect();
        let c = VersionConstraint::parse(">=1.2.0, <2.0.0").unwrap();
        let selected = select_maximum(&available, &c).unwrap();
        assert_eq!(selected.to_string(), "1.5.0");
    }

    #[test]
    fn no_satisfying_version_returns_none() {
        let available: Vec<Version> = ["1.0.0", "1.2.0"]
            .iter()
            .map(|s| Version::parse(s).unwrap())
            .collect();
        let c = VersionConstraint::parse(">=2.0.0").unwrap();
        assert!(select_maximum(&available, &c).is_none());
    }

    #[test]
    fn version_to_string() {
        assert_eq!(Version::parse("1.2.3").unwrap().to_string(), "1.2.3");
    }

    // ── New tests ─────────────────────────────────────────────────────────────

    // --- Version::parse edge cases ---

    #[test]
    fn parse_version_with_leading_whitespace() {
        // Version::parse trims the input
        let v = Version::parse("  1.2.3  ").unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
    }

    #[test]
    fn parse_version_zero_zero_zero() {
        let v = Version::parse("0.0.0").unwrap();
        assert_eq!(v.major, 0);
        assert_eq!(v.minor, 0);
        assert_eq!(v.patch, 0);
    }

    #[test]
    fn parse_version_large_numbers() {
        let v = Version::parse("99.999.9999").unwrap();
        assert_eq!(v.major, 99);
        assert_eq!(v.minor, 999);
        assert_eq!(v.patch, 9999);
    }

    #[test]
    fn parse_version_negative_component_returns_none() {
        // Negative numbers are not valid semver components
        assert!(Version::parse("-1.0.0").is_none());
        assert!(Version::parse("1.-2.0").is_none());
    }

    #[test]
    fn parse_version_non_numeric_component_returns_none() {
        assert!(Version::parse("1.2.x").is_none());
        assert!(Version::parse("a.b.c").is_none());
    }

    // --- Version ordering edge cases ---

    #[test]
    fn version_patch_beats_minor_for_same_major() {
        let v010 = Version::parse("0.1.0").unwrap();
        let v001 = Version::parse("0.0.1").unwrap();
        assert!(v001 < v010);
    }

    #[test]
    fn version_equality() {
        let a = Version::parse("1.2.3").unwrap();
        let b = Version::parse("1.2.3").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn version_inequality() {
        let a = Version::parse("1.2.3").unwrap();
        let b = Version::parse("1.2.4").unwrap();
        assert_ne!(a, b);
    }

    // --- Predicate variants ---

    #[test]
    fn constraint_gt_strict_excludes_boundary() {
        let c = VersionConstraint::parse(">1.0.0").unwrap();
        assert!(
            !c.matches(&Version::parse("1.0.0").unwrap()),
            "1.0.0 not > 1.0.0"
        );
        assert!(
            c.matches(&Version::parse("1.0.1").unwrap()),
            "1.0.1 > 1.0.0"
        );
    }

    #[test]
    fn constraint_lte_includes_boundary() {
        let c = VersionConstraint::parse("<=2.0.0").unwrap();
        assert!(
            c.matches(&Version::parse("2.0.0").unwrap()),
            "2.0.0 <= 2.0.0"
        );
        assert!(
            c.matches(&Version::parse("1.9.9").unwrap()),
            "1.9.9 <= 2.0.0"
        );
        assert!(
            !c.matches(&Version::parse("2.0.1").unwrap()),
            "2.0.1 not <= 2.0.0"
        );
    }

    #[test]
    fn constraint_lt_excludes_boundary() {
        let c = VersionConstraint::parse("<2.0.0").unwrap();
        assert!(
            !c.matches(&Version::parse("2.0.0").unwrap()),
            "2.0.0 not < 2.0.0"
        );
        assert!(
            c.matches(&Version::parse("1.9.9").unwrap()),
            "1.9.9 < 2.0.0"
        );
    }

    #[test]
    fn constraint_eq_explicit_operator() {
        let c = VersionConstraint::parse("=1.5.0").unwrap();
        assert!(c.matches(&Version::parse("1.5.0").unwrap()));
        assert!(!c.matches(&Version::parse("1.5.1").unwrap()));
        assert!(!c.matches(&Version::parse("1.4.9").unwrap()));
    }

    #[test]
    fn constraint_parse_returns_none_on_bad_version() {
        // Invalid version in the constraint → None
        assert!(VersionConstraint::parse(">=not.a.version").is_none());
        assert!(VersionConstraint::parse(">=1.2").is_none());
    }

    // --- select_minimum / select_maximum edge cases ---

    #[test]
    fn select_minimum_empty_list_returns_none() {
        let c = VersionConstraint::parse(">=1.0.0").unwrap();
        assert!(select_minimum(&[], &c).is_none());
    }

    #[test]
    fn select_maximum_empty_list_returns_none() {
        let c = VersionConstraint::parse(">=1.0.0").unwrap();
        assert!(select_maximum(&[], &c).is_none());
    }

    #[test]
    fn select_minimum_single_matching_element() {
        let available = vec![Version::parse("1.0.0").unwrap()];
        let c = VersionConstraint::parse(">=1.0.0").unwrap();
        assert_eq!(select_minimum(&available, &c).unwrap().to_string(), "1.0.0");
    }

    #[test]
    fn select_maximum_single_matching_element() {
        let available = vec![Version::parse("1.0.0").unwrap()];
        let c = VersionConstraint::parse(">=1.0.0").unwrap();
        assert_eq!(select_maximum(&available, &c).unwrap().to_string(), "1.0.0");
    }

    #[test]
    fn select_minimum_unsorted_input_still_correct() {
        // select_minimum should not depend on list order
        let available: Vec<Version> = ["2.0.0", "1.0.0", "1.5.0"]
            .iter()
            .map(|s| Version::parse(s).unwrap())
            .collect();
        let c = VersionConstraint::parse(">=1.0.0, <2.0.0").unwrap();
        assert_eq!(select_minimum(&available, &c).unwrap().to_string(), "1.0.0");
    }

    #[test]
    fn select_maximum_unsorted_input_still_correct() {
        let available: Vec<Version> = ["2.0.0", "1.0.0", "1.5.0"]
            .iter()
            .map(|s| Version::parse(s).unwrap())
            .collect();
        let c = VersionConstraint::parse(">=1.0.0, <2.0.0").unwrap();
        assert_eq!(select_maximum(&available, &c).unwrap().to_string(), "1.5.0");
    }

    // --- Version Display ---

    #[test]
    fn version_display_zero() {
        assert_eq!(Version::parse("0.0.0").unwrap().to_string(), "0.0.0");
    }

    // --- ^ (caret) operator ---

    #[test]
    fn caret_normal_version_allows_minor_and_patch() {
        let c = VersionConstraint::parse("^1.2.3").unwrap();
        assert!(
            c.matches(&Version::parse("1.2.3").unwrap()),
            "lower bound inclusive"
        );
        assert!(c.matches(&Version::parse("1.9.9").unwrap()), "within major");
        assert!(
            !c.matches(&Version::parse("2.0.0").unwrap()),
            "next major excluded"
        );
        assert!(
            !c.matches(&Version::parse("1.2.2").unwrap()),
            "below lower bound"
        );
    }

    #[test]
    fn caret_zero_major_locks_minor() {
        // ^0.2.3 = >=0.2.3, <0.3.0
        let c = VersionConstraint::parse("^0.2.3").unwrap();
        assert!(c.matches(&Version::parse("0.2.3").unwrap()));
        assert!(c.matches(&Version::parse("0.2.9").unwrap()));
        assert!(
            !c.matches(&Version::parse("0.3.0").unwrap()),
            "next minor excluded for 0.x"
        );
        assert!(!c.matches(&Version::parse("1.0.0").unwrap()));
    }

    #[test]
    fn caret_zero_zero_minor_locks_patch() {
        // ^0.0.3 = >=0.0.3, <0.0.4
        let c = VersionConstraint::parse("^0.0.3").unwrap();
        assert!(c.matches(&Version::parse("0.0.3").unwrap()));
        assert!(
            !c.matches(&Version::parse("0.0.4").unwrap()),
            "next patch excluded for 0.0.x"
        );
        assert!(!c.matches(&Version::parse("0.1.0").unwrap()));
    }

    #[test]
    fn caret_selects_maximum_within_major() {
        let available: Vec<Version> = ["1.0.0", "1.2.3", "1.9.0", "2.0.0"]
            .iter()
            .map(|s| Version::parse(s).unwrap())
            .collect();
        let c = VersionConstraint::parse("^1.2.3").unwrap();
        assert_eq!(select_maximum(&available, &c).unwrap().to_string(), "1.9.0");
    }

    // --- ~ (tilde) operator ---

    #[test]
    fn tilde_allows_patch_only() {
        let c = VersionConstraint::parse("~1.2.3").unwrap();
        assert!(
            c.matches(&Version::parse("1.2.3").unwrap()),
            "lower bound inclusive"
        );
        assert!(
            c.matches(&Version::parse("1.2.9").unwrap()),
            "patch bump ok"
        );
        assert!(
            !c.matches(&Version::parse("1.3.0").unwrap()),
            "minor bump excluded"
        );
        assert!(
            !c.matches(&Version::parse("2.0.0").unwrap()),
            "major bump excluded"
        );
        assert!(
            !c.matches(&Version::parse("1.2.2").unwrap()),
            "below lower bound"
        );
    }

    #[test]
    fn tilde_zero_major() {
        // ~0.2.3 = >=0.2.3, <0.3.0
        let c = VersionConstraint::parse("~0.2.3").unwrap();
        assert!(c.matches(&Version::parse("0.2.3").unwrap()));
        assert!(c.matches(&Version::parse("0.2.99").unwrap()));
        assert!(!c.matches(&Version::parse("0.3.0").unwrap()));
    }

    #[test]
    fn tilde_selects_maximum_within_minor() {
        let available: Vec<Version> = ["1.2.3", "1.2.5", "1.3.0", "2.0.0"]
            .iter()
            .map(|s| Version::parse(s).unwrap())
            .collect();
        let c = VersionConstraint::parse("~1.2.3").unwrap();
        assert_eq!(select_maximum(&available, &c).unwrap().to_string(), "1.2.5");
    }

    #[test]
    fn caret_invalid_version_returns_none() {
        assert!(VersionConstraint::parse("^not.a.version").is_none());
        assert!(VersionConstraint::parse("^1.2").is_none());
    }

    #[test]
    fn tilde_invalid_version_returns_none() {
        assert!(VersionConstraint::parse("~not.a.version").is_none());
        assert!(VersionConstraint::parse("~1.2").is_none());
    }
}
