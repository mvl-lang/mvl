// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Global package-manager configuration (read from XDG config) and shared
//! semver-tag selection helpers.

use super::version;

/// Global config loaded from `$XDG_CONFIG_HOME/mvl/config.toml`.
#[derive(Default)]
pub(super) struct GlobalConfig {
    /// Global `min-age-days` default (overridden by project-level `[security]`).
    pub(super) min_age_days: u64,
    /// Global exclusion lists keyed by git URL.
    pub(super) exclusions: std::collections::HashMap<String, Vec<String>>,
}

impl GlobalConfig {
    pub(super) fn load() -> Self {
        let config_dir = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|h| std::path::PathBuf::from(h).join(".config"))
            })
            .unwrap_or_else(|| std::path::PathBuf::from(".config"));
        let path = config_dir.join("mvl").join("config.toml");
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Self::default(),
        };
        Self::parse(&content)
    }

    pub(super) fn parse(content: &str) -> Self {
        let mut min_age_days = 0u64;
        let mut exclusions: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        let mut current_section = String::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                current_section = line[1..line.len() - 1].trim().to_string();
                continue;
            }
            if let Some(eq) = line.find('=') {
                let key = line[..eq].trim().trim_matches('"');
                let val = line[eq + 1..].trim();
                match current_section.as_str() {
                    "security" if key == "min-age-days" => {
                        if let Ok(n) = val.parse::<u64>() {
                            min_age_days = n;
                        }
                    }
                    "exclusions" => {
                        // key = ["ver1", "ver2"]
                        let git_url = key.to_string();
                        let versions = parse_string_array(val);
                        exclusions.insert(git_url, versions);
                    }
                    _ => {}
                }
            }
        }
        Self {
            min_age_days,
            exclusions,
        }
    }
}

fn parse_string_array(s: &str) -> Vec<String> {
    let s = s.trim();
    if !s.starts_with('[') || !s.ends_with(']') {
        return vec![];
    }
    let inner = &s[1..s.len() - 1];
    inner
        .split(',')
        .filter_map(|part| {
            let p = part.trim();
            if p.starts_with('"') && p.ends_with('"') && p.len() >= 2 {
                Some(p[1..p.len() - 1].to_string())
            } else {
                None
            }
        })
        .collect()
}

/// Return the latest tag that parses as a semver version (with optional `v` prefix).
pub(super) fn latest_semver_tag(tags: &[String]) -> Option<String> {
    use version::Version;
    let mut best: Option<(Version, String)> = None;
    for tag in tags {
        let vstr = tag.strip_prefix('v').unwrap_or(tag);
        if let Some(v) = Version::parse(vstr) {
            if best.as_ref().map(|(bv, _)| &v > bv).unwrap_or(true) {
                best = Some((v, tag.clone()));
            }
        }
    }
    best.map(|(_, tag)| tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn latest_semver_tag_empty_list_returns_none() {
        assert_eq!(latest_semver_tag(&[]), None);
    }

    #[test]
    fn latest_semver_tag_picks_highest() {
        let t = tags(&["v0.1.0", "v0.2.0", "v0.1.5"]);
        assert_eq!(latest_semver_tag(&t).as_deref(), Some("v0.2.0"));
    }

    #[test]
    fn latest_semver_tag_ignores_non_semver_entries() {
        let t = tags(&["latest", "v0.1.0", "main", "v0.2.0"]);
        assert_eq!(latest_semver_tag(&t).as_deref(), Some("v0.2.0"));
    }

    #[test]
    fn latest_semver_tag_all_non_semver_returns_none() {
        let t = tags(&["latest", "main", "develop"]);
        assert_eq!(latest_semver_tag(&t), None);
    }

    #[test]
    fn latest_semver_tag_without_v_prefix() {
        let t = tags(&["0.1.0", "0.2.0"]);
        assert_eq!(latest_semver_tag(&t).as_deref(), Some("0.2.0"));
    }

    #[test]
    fn latest_semver_tag_mixed_v_prefix() {
        let t = tags(&["0.1.0", "v0.2.0"]);
        assert_eq!(latest_semver_tag(&t).as_deref(), Some("v0.2.0"));
    }

    #[test]
    fn latest_semver_tag_single_entry() {
        let t = tags(&["v0.1.0"]);
        assert_eq!(latest_semver_tag(&t).as_deref(), Some("v0.1.0"));
    }

    #[test]
    fn latest_semver_tag_preserves_original_tag_string() {
        let t = tags(&["v0.1.0"]);
        assert_eq!(latest_semver_tag(&t).as_deref(), Some("v0.1.0"));
        let t2 = tags(&["0.1.0"]);
        assert_eq!(latest_semver_tag(&t2).as_deref(), Some("0.1.0"));
    }
}
