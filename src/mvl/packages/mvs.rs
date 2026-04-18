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
}
