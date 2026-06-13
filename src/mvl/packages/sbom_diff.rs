// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! SBOM diff and trust half-life tracking.
//!
//! Implements issue #636: `mvl sbom snapshot` / `mvl sbom diff`.
//!
//! Baseline metadata is stored in `.mvl/sbom.baseline.meta` alongside the
//! full CycloneDX snapshot in `.mvl/sbom.baseline.json`.  The meta file uses
//! a simple `key=value` text format so we avoid an external JSON parser.

use std::collections::HashMap;

// ── Dep entry ────────────────────────────────────────────────────────────────

/// Classification of a dependency in the baseline/current dep list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Mvl,
    Native,
    CNative,
}

impl DepKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DepKind::Mvl => "mvl",
            DepKind::Native => "native",
            DepKind::CNative => "c-native",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "mvl" => Some(DepKind::Mvl),
            "native" => Some(DepKind::Native),
            "c-native" | "c_native" => Some(DepKind::CNative),
            _ => None,
        }
    }
}

/// A single dependency recorded in the baseline or current state.
#[derive(Debug, Clone)]
pub struct DepEntry {
    pub name: String,
    pub version: String,
    pub kind: DepKind,
}

// ── Baseline meta ─────────────────────────────────────────────────────────────

/// Lightweight baseline metadata stored in `.mvl/sbom.baseline.meta`.
///
/// Format (one `key=value` per line):
/// ```text
/// timestamp_secs=1749816000
/// half_life_days=90
/// trust_score=10.0000
/// source_count=12
/// mvl=foo:1.0.0|bar:2.0.0
/// native=tokio:1.37.0|hyper:1.5.1
/// c_native=openssl:3.1.0
/// ```
pub struct BaselineMeta {
    pub timestamp_secs: u64,
    pub half_life_days: f64,
    pub trust_score: f64,
    pub deps: Vec<DepEntry>,
    pub source_count: usize,
}

impl BaselineMeta {
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        out += &format!("timestamp_secs={}\n", self.timestamp_secs);
        out += &format!("half_life_days={}\n", self.half_life_days);
        out += &format!("trust_score={:.4}\n", self.trust_score);
        out += &format!("source_count={}\n", self.source_count);

        let mvl: Vec<_> = self
            .deps
            .iter()
            .filter(|d| d.kind == DepKind::Mvl)
            .map(|d| format!("{}:{}", d.name, d.version))
            .collect();
        if !mvl.is_empty() {
            out += &format!("mvl={}\n", mvl.join("|"));
        }
        let native: Vec<_> = self
            .deps
            .iter()
            .filter(|d| d.kind == DepKind::Native)
            .map(|d| format!("{}:{}", d.name, d.version))
            .collect();
        if !native.is_empty() {
            out += &format!("native={}\n", native.join("|"));
        }
        let cnative: Vec<_> = self
            .deps
            .iter()
            .filter(|d| d.kind == DepKind::CNative)
            .map(|d| format!("{}:{}", d.name, d.version))
            .collect();
        if !cnative.is_empty() {
            out += &format!("c_native={}\n", cnative.join("|"));
        }
        out
    }

    pub fn parse(content: &str) -> Result<Self, String> {
        let mut timestamp_secs: u64 = 0;
        let mut half_life_days: f64 = 90.0;
        let mut trust_score: f64 = 10.0;
        let mut source_count: usize = 0;
        let mut deps: Vec<DepEntry> = Vec::new();

        for line in content.lines() {
            let Some((key, val)) = line.split_once('=') else {
                continue;
            };
            match key {
                "timestamp_secs" => {
                    timestamp_secs = val
                        .parse()
                        .map_err(|_| "invalid timestamp_secs".to_string())?;
                }
                "half_life_days" => {
                    half_life_days = val
                        .parse()
                        .map_err(|_| "invalid half_life_days".to_string())?;
                }
                "trust_score" => {
                    trust_score = val.parse().map_err(|_| "invalid trust_score".to_string())?;
                }
                "source_count" => {
                    source_count = val
                        .parse()
                        .map_err(|_| "invalid source_count".to_string())?;
                }
                kind_key @ ("mvl" | "native" | "c_native") => {
                    let kind = DepKind::parse(kind_key).unwrap();
                    for entry in val.split('|').filter(|s| !s.is_empty()) {
                        if let Some((name, version)) = entry.split_once(':') {
                            deps.push(DepEntry {
                                name: name.to_string(),
                                version: version.to_string(),
                                kind,
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        if timestamp_secs == 0 {
            return Err("missing or zero timestamp_secs".to_string());
        }

        Ok(BaselineMeta {
            timestamp_secs,
            half_life_days,
            trust_score,
            deps,
            source_count,
        })
    }
}

// ── Diff result ───────────────────────────────────────────────────────────────

/// A single dependency that was added, removed, or updated.
#[derive(Debug)]
pub struct DepChange {
    pub name: String,
    pub old_version: Option<String>,
    pub new_version: Option<String>,
    pub kind: DepKind,
}

/// Result of comparing the current SBOM state against a baseline snapshot.
pub struct SbomDiff {
    pub baseline_date: String,
    pub current_date: String,
    pub days_elapsed: u64,
    pub added: Vec<DepChange>,
    pub removed: Vec<DepChange>,
    pub updated: Vec<DepChange>,
    pub source_count_baseline: usize,
    pub source_count_current: usize,
    pub baseline_trust: f64,
    pub current_trust: f64,
    pub half_life_days: f64,
}

impl SbomDiff {
    /// Compute the diff between `meta` (baseline) and the current dep list.
    pub fn compute(
        meta: &BaselineMeta,
        current_deps: &[DepEntry],
        current_source_count: usize,
        current_secs: u64,
    ) -> Self {
        let days_elapsed = current_secs.saturating_sub(meta.timestamp_secs) / 86400;

        let baseline_map: HashMap<&str, (&str, DepKind)> = meta
            .deps
            .iter()
            .map(|d| (d.name.as_str(), (d.version.as_str(), d.kind)))
            .collect();
        let current_map: HashMap<&str, (&str, DepKind)> = current_deps
            .iter()
            .map(|d| (d.name.as_str(), (d.version.as_str(), d.kind)))
            .collect();

        let mut added = Vec::new();
        let mut updated = Vec::new();
        for d in current_deps {
            match baseline_map.get(d.name.as_str()) {
                None => added.push(DepChange {
                    name: d.name.clone(),
                    old_version: None,
                    new_version: Some(d.version.clone()),
                    kind: d.kind,
                }),
                Some((old_ver, _)) if *old_ver != d.version.as_str() => {
                    updated.push(DepChange {
                        name: d.name.clone(),
                        old_version: Some(old_ver.to_string()),
                        new_version: Some(d.version.clone()),
                        kind: d.kind,
                    });
                }
                _ => {}
            }
        }
        let mut removed = Vec::new();
        for d in &meta.deps {
            if !current_map.contains_key(d.name.as_str()) {
                removed.push(DepChange {
                    name: d.name.clone(),
                    old_version: Some(d.version.clone()),
                    new_version: None,
                    kind: d.kind,
                });
            }
        }

        let current_trust = compute_trust(
            meta.trust_score,
            days_elapsed,
            meta.half_life_days,
            &added,
            &removed,
            &updated,
        );

        SbomDiff {
            baseline_date: format_date(meta.timestamp_secs),
            current_date: format_date(current_secs),
            days_elapsed,
            added,
            removed,
            updated,
            source_count_baseline: meta.source_count,
            source_count_current: current_source_count,
            baseline_trust: meta.trust_score,
            current_trust,
            half_life_days: meta.half_life_days,
        }
    }

    /// Returns `true` if the trust score regressed by more than `threshold`.
    pub fn has_regression(&self, threshold: f64) -> bool {
        self.baseline_trust - self.current_trust > threshold
    }

    /// Human-readable output (default).
    pub fn render(&self) -> String {
        let mut out = String::new();
        out += &format!(
            "SBOM diff: {} → {} ({} days)\n\n",
            self.baseline_date, self.current_date, self.days_elapsed
        );

        if self.added.is_empty()
            && self.removed.is_empty()
            && self.updated.is_empty()
            && self.source_count_baseline == self.source_count_current
        {
            out += "  No changes.\n\n";
        } else {
            if !self.added.is_empty() {
                out += &format!("  Added ({}):\n", self.added.len());
                for d in &self.added {
                    out += &format!(
                        "    + {}  {}   [{}]\n",
                        d.name,
                        d.new_version.as_deref().unwrap_or("?"),
                        d.kind.as_str()
                    );
                }
                out += "\n";
            }
            if !self.updated.is_empty() {
                out += &format!("  Updated ({}):\n", self.updated.len());
                for d in &self.updated {
                    out += &format!(
                        "    ~ {}  {} → {}   [{}]\n",
                        d.name,
                        d.old_version.as_deref().unwrap_or("?"),
                        d.new_version.as_deref().unwrap_or("?"),
                        d.kind.as_str()
                    );
                }
                out += "\n";
            }
            if !self.removed.is_empty() {
                out += &format!("  Removed ({}):\n", self.removed.len());
                for d in &self.removed {
                    out += &format!(
                        "    - {}  {}   [{}]\n",
                        d.name,
                        d.old_version.as_deref().unwrap_or("?"),
                        d.kind.as_str()
                    );
                }
                out += "\n";
            }
            if self.source_count_baseline != self.source_count_current {
                let delta = self.source_count_current as i64 - self.source_count_baseline as i64;
                out += &format!(
                    "  Source files: {} → {} ({:+})\n\n",
                    self.source_count_baseline, self.source_count_current, delta
                );
            }
        }

        let delta = self.current_trust - self.baseline_trust;
        let sign = if delta >= 0.0 { "+" } else { "" };
        out += &format!(
            "  Trust score: {:.1}/10 → {:.1}/10  ({}{:.1})\n",
            self.baseline_trust, self.current_trust, sign, delta
        );
        out += "\n";
        out += &format!(
            "  Trust half-life: {:.1}/10  (snapshot age: {} days, half-life: {} days)\n",
            self.current_trust, self.days_elapsed, self.half_life_days as u64
        );

        // Recommend next audit when score would drop below 7.0
        if self.current_trust > 7.0 && self.half_life_days > 0.0 {
            let ratio = self.current_trust / 7.0;
            if ratio > 1.0 {
                let days_to_threshold = (self.half_life_days * ratio.log2()).ceil() as u64;
                out += &format!("  Next recommended audit: in ~{} days\n", days_to_threshold);
            }
        }

        out
    }

    /// Machine-readable JSON output for CI tools.
    pub fn render_json(&self) -> String {
        let mut out = String::new();
        out += "{\n";
        out += &format!("  \"baseline_date\": \"{}\",\n", self.baseline_date);
        out += &format!("  \"current_date\": \"{}\",\n", self.current_date);
        out += &format!("  \"days_elapsed\": {},\n", self.days_elapsed);

        out += "  \"added\": [\n";
        for (i, d) in self.added.iter().enumerate() {
            let comma = if i + 1 < self.added.len() { "," } else { "" };
            out += &format!(
                "    {{\"name\": \"{}\", \"version\": \"{}\", \"kind\": \"{}\"}}{}\n",
                d.name,
                d.new_version.as_deref().unwrap_or(""),
                d.kind.as_str(),
                comma
            );
        }
        out += "  ],\n";

        out += "  \"removed\": [\n";
        for (i, d) in self.removed.iter().enumerate() {
            let comma = if i + 1 < self.removed.len() { "," } else { "" };
            out += &format!(
                "    {{\"name\": \"{}\", \"version\": \"{}\", \"kind\": \"{}\"}}{}\n",
                d.name,
                d.old_version.as_deref().unwrap_or(""),
                d.kind.as_str(),
                comma
            );
        }
        out += "  ],\n";

        out += "  \"updated\": [\n";
        for (i, d) in self.updated.iter().enumerate() {
            let comma = if i + 1 < self.updated.len() { "," } else { "" };
            out += &format!(
                "    {{\"name\": \"{}\", \"from\": \"{}\", \"to\": \"{}\", \"kind\": \"{}\"}}{}\n",
                d.name,
                d.old_version.as_deref().unwrap_or(""),
                d.new_version.as_deref().unwrap_or(""),
                d.kind.as_str(),
                comma
            );
        }
        out += "  ],\n";

        out += &format!(
            "  \"source_count_baseline\": {},\n",
            self.source_count_baseline
        );
        out += &format!(
            "  \"source_count_current\": {},\n",
            self.source_count_current
        );
        out += &format!("  \"trust_baseline\": {:.4},\n", self.baseline_trust);
        out += &format!("  \"trust_current\": {:.4},\n", self.current_trust);
        out += &format!("  \"half_life_days\": {}\n", self.half_life_days as u64);
        out += "}\n";
        out
    }
}

// ── Trust scoring ─────────────────────────────────────────────────────────────

fn compute_trust(
    baseline: f64,
    days: u64,
    half_life: f64,
    added: &[DepChange],
    removed: &[DepChange],
    updated: &[DepChange],
) -> f64 {
    // Exponential time decay
    let decay = 0.5f64.powf(days as f64 / half_life);
    let mut score = baseline * decay;

    // Each new dep reduces trust (C > native > mvl due to attack surface)
    for d in added {
        score -= match d.kind {
            DepKind::CNative => 0.5,
            DepKind::Native => 0.3,
            DepKind::Mvl => 0.1,
        };
    }
    // Removing a dep slightly improves trust (reduced surface)
    score += removed.len() as f64 * 0.05;
    // Updating a dep slightly improves trust (active maintenance)
    score += updated.len() as f64 * 0.02;

    score.clamp(0.0, 10.0)
}

// ── Utilities ────────────────────────────────────────────────────────────────

/// Format Unix seconds as `YYYY-MM-DD` using Hinnant's civil calendar algorithm.
fn format_date(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Current Unix timestamp in seconds.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_meta_roundtrip() {
        let meta = BaselineMeta {
            timestamp_secs: 1_749_816_000,
            half_life_days: 90.0,
            trust_score: 10.0,
            source_count: 5,
            deps: vec![
                DepEntry {
                    name: "foo".to_string(),
                    version: "1.0.0".to_string(),
                    kind: DepKind::Mvl,
                },
                DepEntry {
                    name: "tokio".to_string(),
                    version: "1.37.0".to_string(),
                    kind: DepKind::Native,
                },
                DepEntry {
                    name: "openssl".to_string(),
                    version: "3.1.0".to_string(),
                    kind: DepKind::CNative,
                },
            ],
        };
        let ser = meta.serialize();
        let parsed = BaselineMeta::parse(&ser).unwrap();
        assert_eq!(parsed.timestamp_secs, 1_749_816_000);
        assert_eq!(parsed.half_life_days, 90.0);
        assert_eq!(parsed.trust_score, 10.0);
        assert_eq!(parsed.source_count, 5);
        assert_eq!(parsed.deps.len(), 3);
        assert_eq!(parsed.deps[0].name, "foo");
        assert_eq!(parsed.deps[0].kind, DepKind::Mvl);
        assert_eq!(parsed.deps[1].name, "tokio");
        assert_eq!(parsed.deps[1].kind, DepKind::Native);
        assert_eq!(parsed.deps[2].name, "openssl");
        assert_eq!(parsed.deps[2].kind, DepKind::CNative);
    }

    #[test]
    fn baseline_meta_parse_error_on_missing_timestamp() {
        let result = BaselineMeta::parse("half_life_days=90\ntrust_score=10.0\nsource_count=0\n");
        assert!(result.is_err());
    }

    #[test]
    fn diff_detects_added_removed_updated() {
        let meta = BaselineMeta {
            timestamp_secs: 1_749_816_000,
            half_life_days: 90.0,
            trust_score: 10.0,
            source_count: 3,
            deps: vec![
                DepEntry {
                    name: "foo".to_string(),
                    version: "1.0.0".to_string(),
                    kind: DepKind::Mvl,
                },
                DepEntry {
                    name: "bar".to_string(),
                    version: "2.0.0".to_string(),
                    kind: DepKind::Native,
                },
            ],
        };
        let current = vec![
            DepEntry {
                name: "foo".to_string(),
                version: "1.1.0".to_string(),
                kind: DepKind::Mvl,
            }, // updated
            DepEntry {
                name: "baz".to_string(),
                version: "0.1.0".to_string(),
                kind: DepKind::CNative,
            }, // added
               // bar removed
        ];
        let diff = SbomDiff::compute(&meta, &current, 3, 1_749_816_000 + 7 * 86400);
        assert_eq!(diff.updated.len(), 1);
        assert_eq!(diff.updated[0].name, "foo");
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.added[0].name, "baz");
        assert_eq!(diff.removed.len(), 1);
        assert_eq!(diff.removed[0].name, "bar");
    }

    #[test]
    fn trust_decays_over_time() {
        let meta = BaselineMeta {
            timestamp_secs: 0,
            half_life_days: 90.0,
            trust_score: 10.0,
            source_count: 0,
            deps: vec![],
        };
        // After exactly one half-life with no changes, score should be ~5.0
        let diff = SbomDiff::compute(&meta, &[], 0, 90 * 86400);
        assert!((diff.current_trust - 5.0).abs() < 0.01);
    }

    #[test]
    fn has_regression_threshold() {
        let meta = BaselineMeta {
            timestamp_secs: 0,
            half_life_days: 90.0,
            trust_score: 10.0,
            source_count: 0,
            deps: vec![],
        };
        // No time elapsed, no changes → no regression
        let diff = SbomDiff::compute(&meta, &[], 0, 0);
        assert!(!diff.has_regression(0.5));

        // After 90 days (half-life), score drops by 5.0 → regression
        let diff = SbomDiff::compute(&meta, &[], 0, 90 * 86400);
        assert!(diff.has_regression(0.5));
    }

    #[test]
    fn format_date_epoch() {
        assert_eq!(format_date(0), "1970-01-01");
    }

    #[test]
    fn format_date_known() {
        // 2026-06-13 = 20617 days since epoch
        assert_eq!(format_date(20617 * 86400), "2026-06-13");
    }

    #[test]
    fn render_json_valid_structure() {
        let meta = BaselineMeta {
            timestamp_secs: 1_749_816_000,
            half_life_days: 90.0,
            trust_score: 10.0,
            source_count: 2,
            deps: vec![DepEntry {
                name: "foo".to_string(),
                version: "1.0.0".to_string(),
                kind: DepKind::Mvl,
            }],
        };
        let current = vec![DepEntry {
            name: "foo".to_string(),
            version: "1.0.0".to_string(),
            kind: DepKind::Mvl,
        }];
        let diff = SbomDiff::compute(&meta, &current, 2, 1_749_816_000 + 3 * 86400);
        let json = diff.render_json();
        assert!(json.contains("\"added\": ["));
        assert!(json.contains("\"removed\": ["));
        assert!(json.contains("\"updated\": ["));
        assert!(json.contains("\"trust_current\""));
        assert!(json.starts_with('{'));
        assert!(json.trim_end().ends_with('}'));
    }
}
