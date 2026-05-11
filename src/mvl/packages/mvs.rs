// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Minimum Version Selection (MVS) algorithm.
//!
//! Go's MVS selects the *minimum* version of each dependency that satisfies
//! all constraints declared across the project and its transitive deps.
//! This is simpler and more reproducible than SAT-based resolution.
//!
//! Reference: https://research.swtch.com/vgo-mvs

use crate::mvl::packages::version::Version;
use std::collections::HashMap;

/// A dependency requirement: package name + minimum version required.
#[derive(Debug, Clone)]
pub struct Requirement {
    pub name: String,
    pub min_version: Version,
}

/// Build list: the selected version for each package.
pub type BuildList = HashMap<String, Version>;

/// Apply MVS: given a flat list of requirements (possibly with duplicates for
/// the same package), select the *maximum* of all minimum-required versions
/// for each package.
///
/// This is the core MVS invariant: if module A requires `foo >= 1.2` and
/// module B requires `foo >= 1.4`, the build list uses `foo 1.4` — the
/// minimum that satisfies everyone.
pub fn resolve(requirements: &[Requirement]) -> BuildList {
    let mut selected: HashMap<String, Version> = HashMap::new();
    for req in requirements {
        let entry = selected
            .entry(req.name.clone())
            .or_insert_with(|| req.min_version.clone());
        if req.min_version > *entry {
            *entry = req.min_version.clone();
        }
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    // ── Existing tests ────────────────────────────────────────────────────────

    #[test]
    fn single_requirement() {
        let reqs = vec![Requirement {
            name: "http".to_string(),
            min_version: v("1.2.0"),
        }];
        let build = resolve(&reqs);
        assert_eq!(build["http"].to_string(), "1.2.0");
    }

    #[test]
    fn conflicting_requirements_picks_maximum_minimum() {
        let reqs = vec![
            Requirement {
                name: "http".to_string(),
                min_version: v("1.2.0"),
            },
            Requirement {
                name: "http".to_string(),
                min_version: v("1.4.0"),
            },
            Requirement {
                name: "http".to_string(),
                min_version: v("1.3.0"),
            },
        ];
        let build = resolve(&reqs);
        // MVS: select max of all mins = 1.4.0
        assert_eq!(build["http"].to_string(), "1.4.0");
    }

    #[test]
    fn multiple_packages_independent() {
        let reqs = vec![
            Requirement {
                name: "http".to_string(),
                min_version: v("1.0.0"),
            },
            Requirement {
                name: "tls".to_string(),
                min_version: v("0.4.0"),
            },
            Requirement {
                name: "http".to_string(),
                min_version: v("1.2.0"),
            },
        ];
        let build = resolve(&reqs);
        assert_eq!(build["http"].to_string(), "1.2.0");
        assert_eq!(build["tls"].to_string(), "0.4.0");
    }

    #[test]
    fn empty_requirements() {
        let build = resolve(&[]);
        assert!(build.is_empty());
    }

    // ── New tests ─────────────────────────────────────────────────────────────

    #[test]
    fn single_package_same_version_twice_deduplicates() {
        let reqs = vec![
            Requirement {
                name: "foo".to_string(),
                min_version: v("1.0.0"),
            },
            Requirement {
                name: "foo".to_string(),
                min_version: v("1.0.0"),
            },
        ];
        let build = resolve(&reqs);
        assert_eq!(build.len(), 1);
        assert_eq!(build["foo"].to_string(), "1.0.0");
    }

    #[test]
    fn mvs_result_is_deterministic_regardless_of_input_order() {
        let make_reqs = |order: &[(&str, &str)]| {
            order
                .iter()
                .map(|(name, ver)| Requirement {
                    name: name.to_string(),
                    min_version: v(ver),
                })
                .collect::<Vec<_>>()
        };

        let order_a = make_reqs(&[("pkg", "1.0.0"), ("pkg", "1.5.0"), ("pkg", "1.2.0")]);
        let order_b = make_reqs(&[("pkg", "1.5.0"), ("pkg", "1.2.0"), ("pkg", "1.0.0")]);
        let order_c = make_reqs(&[("pkg", "1.2.0"), ("pkg", "1.0.0"), ("pkg", "1.5.0")]);

        let build_a = resolve(&order_a);
        let build_b = resolve(&order_b);
        let build_c = resolve(&order_c);

        assert_eq!(build_a["pkg"].to_string(), "1.5.0");
        assert_eq!(build_b["pkg"].to_string(), "1.5.0");
        assert_eq!(build_c["pkg"].to_string(), "1.5.0");
    }

    #[test]
    fn package_names_are_case_sensitive() {
        let reqs = vec![
            Requirement {
                name: "Foo".to_string(),
                min_version: v("1.0.0"),
            },
            Requirement {
                name: "foo".to_string(),
                min_version: v("2.0.0"),
            },
        ];
        let build = resolve(&reqs);
        assert_eq!(build.len(), 2, "Foo and foo are different packages");
        assert_eq!(build["Foo"].to_string(), "1.0.0");
        assert_eq!(build["foo"].to_string(), "2.0.0");
    }

    #[test]
    fn mvs_selects_large_version_over_small() {
        let reqs = vec![
            Requirement {
                name: "lib".to_string(),
                min_version: v("1.0.0"),
            },
            Requirement {
                name: "lib".to_string(),
                min_version: v("99.0.0"),
            },
        ];
        let build = resolve(&reqs);
        assert_eq!(build["lib"].to_string(), "99.0.0");
    }

    #[test]
    fn mvs_three_packages_each_with_multiple_requirements() {
        let reqs = vec![
            Requirement {
                name: "a".to_string(),
                min_version: v("1.0.0"),
            },
            Requirement {
                name: "b".to_string(),
                min_version: v("2.0.0"),
            },
            Requirement {
                name: "c".to_string(),
                min_version: v("3.0.0"),
            },
            Requirement {
                name: "a".to_string(),
                min_version: v("1.1.0"),
            },
            Requirement {
                name: "b".to_string(),
                min_version: v("2.1.0"),
            },
            Requirement {
                name: "c".to_string(),
                min_version: v("2.9.9"),
            },
        ];
        let build = resolve(&reqs);
        assert_eq!(build["a"].to_string(), "1.1.0");
        assert_eq!(build["b"].to_string(), "2.1.0");
        // c: max(3.0.0, 2.9.9) = 3.0.0
        assert_eq!(build["c"].to_string(), "3.0.0");
    }

    #[test]
    fn build_list_contains_exactly_the_named_packages() {
        let reqs = vec![
            Requirement {
                name: "x".to_string(),
                min_version: v("1.0.0"),
            },
            Requirement {
                name: "y".to_string(),
                min_version: v("2.0.0"),
            },
        ];
        let build = resolve(&reqs);
        assert_eq!(build.len(), 2);
        assert!(build.contains_key("x"));
        assert!(build.contains_key("y"));
        assert!(!build.contains_key("z"));
    }
}
