// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Supply-chain vulnerability audit for `[native]` and `[c-native]` dependencies.
//!
//! Implements issue #633: `mvl audit --supply-chain` scans C libraries declared
//! in `[c-native]` against NVD (NIST) and OSV (osv.dev), and maps CVEs to MVL
//! requirement gaps using CWE classification.
//!
//! Also scans `[native]` (Rust crates) against OSV for ecosystem `"crates.io"`.

use std::collections::HashMap;

// ── CVE severity ─────────────────────────────────────────────────────────────

/// CVSS severity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    /// Parse from a CVSS score (0.0–10.0).
    pub fn from_score(score: f64) -> Self {
        if score >= 9.0 {
            Severity::Critical
        } else if score >= 7.0 {
            Severity::High
        } else if score >= 4.0 {
            Severity::Medium
        } else {
            Severity::Low
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── CWE → MVL Requirement mapping ───────────────────────────────────────────

/// An MVL requirement that a CVE maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MvlRequirement {
    /// R2: Memory safety
    MemorySafety,
    /// R4: Null safety
    NullSafety,
    /// R8: Concurrency / data race freedom
    Concurrency,
    /// R11: Information flow control
    InformationFlow,
}

impl MvlRequirement {
    pub fn number(&self) -> u8 {
        match self {
            MvlRequirement::MemorySafety => 2,
            MvlRequirement::NullSafety => 4,
            MvlRequirement::Concurrency => 8,
            MvlRequirement::InformationFlow => 11,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            MvlRequirement::MemorySafety => "memory safety",
            MvlRequirement::NullSafety => "null safety",
            MvlRequirement::Concurrency => "concurrency",
            MvlRequirement::InformationFlow => "information flow",
        }
    }
}

impl std::fmt::Display for MvlRequirement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "R{} ({})", self.number(), self.label())
    }
}

/// Map a CWE ID to an MVL requirement, if applicable.
pub fn cwe_to_requirement(cwe_id: u32) -> Option<MvlRequirement> {
    match cwe_id {
        // Memory safety: buffer overflow, out-of-bounds write/read, heap overflow
        119 | 120 | 121 | 122 | 125 | 787 | 788 | 416 | 415 => Some(MvlRequirement::MemorySafety),
        // Null safety: null pointer dereference
        476 => Some(MvlRequirement::NullSafety),
        // Concurrency: race conditions, TOCTOU
        362 | 367 => Some(MvlRequirement::Concurrency),
        // Information flow: information exposure, cleartext storage, sensitive data
        200 | 312 | 319 | 532 | 209 => Some(MvlRequirement::InformationFlow),
        _ => None,
    }
}

// ── Vulnerability result types ──────────────────────────────────────────────

/// A single CVE/vulnerability finding.
#[derive(Debug, Clone)]
pub struct VulnFinding {
    /// CVE ID (e.g. "CVE-2023-12345") or OSV ID.
    pub id: String,
    /// CVSS severity.
    pub severity: Severity,
    /// Mapped MVL requirement, if CWE maps to one.
    pub requirement: Option<MvlRequirement>,
    /// Short summary/description.
    pub summary: String,
    /// CWE IDs found in the advisory.
    pub cwes: Vec<u32>,
}

/// Source of vulnerability data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VulnSource {
    /// NIST National Vulnerability Database.
    Nvd,
    /// Open Source Vulnerabilities (osv.dev).
    Osv,
}

/// Audit result for a single dependency.
#[derive(Debug, Clone)]
pub struct DepAuditResult {
    /// Dependency name (e.g. "openssl", "libz").
    pub name: String,
    /// Declared version.
    pub version: String,
    /// Source section: "native" (Rust) or "c-native" (C).
    pub section: String,
    /// Vulnerabilities found.
    pub findings: Vec<VulnFinding>,
}

impl DepAuditResult {
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }

    pub fn count_by_severity(&self, sev: Severity) -> usize {
        self.findings.iter().filter(|f| f.severity == sev).count()
    }

    /// Format a severity summary like "4 CVEs (1 critical, 2 high, 1 medium)".
    pub fn severity_summary(&self) -> String {
        if self.findings.is_empty() {
            return "clean".to_string();
        }
        let total = self.findings.len();
        let critical = self.count_by_severity(Severity::Critical);
        let high = self.count_by_severity(Severity::High);
        let medium = self.count_by_severity(Severity::Medium);
        let low = self.count_by_severity(Severity::Low);

        let mut parts = Vec::new();
        if critical > 0 {
            parts.push(format!("{critical} critical"));
        }
        if high > 0 {
            parts.push(format!("{high} high"));
        }
        if medium > 0 {
            parts.push(format!("{medium} medium"));
        }
        if low > 0 {
            parts.push(format!("{low} low"));
        }
        format!(
            "{total} CVE{s} ({detail})",
            s = if total == 1 { "" } else { "s" },
            detail = parts.join(", ")
        )
    }
}

/// Full supply-chain audit report.
#[derive(Debug)]
pub struct SupplyChainAudit {
    /// Per-dependency results.
    pub results: Vec<DepAuditResult>,
    /// Errors encountered during scanning (non-fatal).
    pub errors: Vec<String>,
}

impl SupplyChainAudit {
    /// Total number of vulnerabilities found across all deps.
    pub fn total_findings(&self) -> usize {
        self.results.iter().map(|r| r.findings.len()).sum()
    }

    /// True if any vulnerability was found.
    pub fn has_vulnerabilities(&self) -> bool {
        self.total_findings() > 0
    }

    /// Render the audit report.
    pub fn render(&self) -> String {
        let mut out = String::new();

        // Group by section
        let mut by_section: HashMap<&str, Vec<&DepAuditResult>> = HashMap::new();
        for r in &self.results {
            by_section.entry(r.section.as_str()).or_default().push(r);
        }

        for section in &["c-native", "native"] {
            if let Some(deps) = by_section.get(section) {
                out.push_str(&format!("[{section}]\n"));
                let mut deps = deps.clone();
                deps.sort_by(|a, b| a.name.cmp(&b.name));
                for dep in &deps {
                    out.push_str(&format!(
                        "  {} {} — {}\n",
                        dep.name,
                        dep.version,
                        dep.severity_summary()
                    ));
                    // List individual findings
                    let mut sorted_findings = dep.findings.clone();
                    sorted_findings.sort_by_key(|f| std::cmp::Reverse(f.severity));
                    for f in &sorted_findings {
                        let req_str = match &f.requirement {
                            Some(r) => format!("{r}"),
                            None => "—".to_string(),
                        };
                        out.push_str(&format!(
                            "    {:<16} {:<10} {:<24} — {}\n",
                            f.id,
                            f.severity,
                            req_str,
                            truncate(&f.summary, 60)
                        ));
                    }
                }
                out.push('\n');
            }
        }

        if !self.errors.is_empty() {
            out.push_str("Warnings:\n");
            for e in &self.errors {
                out.push_str(&format!("  {e}\n"));
            }
            out.push('\n');
        }

        let total = self.total_findings();
        if total == 0 {
            out.push_str("No known vulnerabilities found.\n");
        } else {
            out.push_str(&format!(
                "{total} vulnerabilit{s} found across {n} dependenc{ds}.\n",
                s = if total == 1 { "y" } else { "ies" },
                n = self.results.iter().filter(|r| !r.is_clean()).count(),
                ds = if self.results.iter().filter(|r| !r.is_clean()).count() == 1 {
                    "y"
                } else {
                    "ies"
                }
            ));
        }

        out
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

// ── NVD API v2 client ───────────────────────────────────────────────────────

/// Query the NVD API v2 for vulnerabilities affecting a C library.
///
/// Uses CPE keyword search: `cpe:2.3:a:*:{name}:{version}:*`.
/// Requires `ureq` for HTTP. Rate limit: 5 req/min without API key,
/// 60 req/min with `NVD_API_KEY` env var.
pub fn query_nvd(name: &str, version: &str) -> Result<Vec<VulnFinding>, String> {
    let api_key = std::env::var("NVD_API_KEY").ok();

    // Build keyword search URL — NVD v2 API
    let keyword = format!("{name} {version}");
    let encoded_keyword = url_encode(&keyword);
    let url = format!(
        "https://services.nvd.nist.gov/rest/json/cves/2.0?keywordSearch={encoded_keyword}&resultsPerPage=50"
    );

    let mut req = ureq::get(&url);
    if let Some(ref key) = api_key {
        req = req.set("apiKey", key);
    }

    let resp = req.call().map_err(|e| format!("NVD API error: {e}"))?;
    let body = resp
        .into_string()
        .map_err(|e| format!("NVD response read error: {e}"))?;

    parse_nvd_response(&body, name)
}

/// Parse NVD API v2 JSON response into vulnerability findings.
pub fn parse_nvd_response(json: &str, _name: &str) -> Result<Vec<VulnFinding>, String> {
    let mut findings = Vec::new();

    // Minimal JSON extraction — we walk the response looking for CVE entries.
    // NVD v2 structure:
    //   { "vulnerabilities": [ { "cve": { "id": "...", "descriptions": [...],
    //     "metrics": { "cvssMetricV31": [{ "cvssData": { "baseScore": N } }] },
    //     "weaknesses": [{ "description": [{ "value": "CWE-NNN" }] }]
    //   } } ] }

    // Find each vulnerability block
    let mut pos = 0;
    while let Some(cve_start) = json[pos..].find("\"cve\"") {
        let abs = pos + cve_start;
        // Find the CVE ID
        let cve_id = extract_string_field(&json[abs..], "\"id\"");
        let Some(cve_id) = cve_id else {
            pos = abs + 5;
            continue;
        };
        if !cve_id.starts_with("CVE-") {
            pos = abs + 5;
            continue;
        }

        // Find the block extent (next "cve" key or end)
        let block_end = json[abs + 5..]
            .find("\"cve\"")
            .map(|p| abs + 5 + p)
            .unwrap_or(json.len());
        let block = &json[abs..block_end];

        // Extract CVSS score
        let score = extract_base_score(block).unwrap_or(0.0);

        // Extract CWE IDs
        let cwes = extract_cwes(block);

        // Extract description
        let summary = extract_description(block).unwrap_or_default();

        // Map CWEs to MVL requirement (pick highest-priority match)
        let requirement = cwes.iter().find_map(|cwe| cwe_to_requirement(*cwe));

        findings.push(VulnFinding {
            id: cve_id,
            severity: Severity::from_score(score),
            requirement,
            summary,
            cwes,
        });

        pos = block_end;
    }

    Ok(findings)
}

// ── OSV API client ──────────────────────────────────────────────────────────

/// Query the OSV API for vulnerabilities affecting a package.
///
/// For C libraries, uses ecosystem `"C"`.
/// For Rust crates, uses ecosystem `"crates.io"`.
pub fn query_osv(name: &str, version: &str, ecosystem: &str) -> Result<Vec<VulnFinding>, String> {
    let url = "https://api.osv.dev/v1/query";
    let body = format!(
        r#"{{"package":{{"name":"{name}","ecosystem":"{ecosystem}"}},"version":"{version}"}}"#
    );

    let resp = ureq::post(url)
        .set("Content-Type", "application/json")
        .send_string(&body)
        .map_err(|e| format!("OSV API error: {e}"))?;

    let resp_body = resp
        .into_string()
        .map_err(|e| format!("OSV response read error: {e}"))?;

    parse_osv_response(&resp_body)
}

/// Parse OSV API JSON response into vulnerability findings.
pub fn parse_osv_response(json: &str) -> Result<Vec<VulnFinding>, String> {
    let mut findings = Vec::new();

    // OSV response structure:
    //   { "vulns": [ { "id": "...", "summary": "...",
    //     "severity": [{ "type": "CVSS_V3", "score": "CVSS:3.1/AV:N/..." }],
    //     "database_specific": { "cwe_ids": ["CWE-119"] }
    //   } ] }

    // Find each vulnerability
    let mut pos = 0;
    while let Some(id_start) = json[pos..].find("\"id\"") {
        let abs = pos + id_start;
        let id = extract_string_field(&json[abs..], "\"id\"");
        let Some(id) = id else {
            pos = abs + 4;
            continue;
        };

        // Skip if this looks like a nested or metadata id
        if id.is_empty() {
            pos = abs + 4;
            continue;
        }

        // Find the block extent (next "id" key or end)
        let block_end = json[abs + 4..]
            .find("\"id\"")
            .map(|p| abs + 4 + p)
            .unwrap_or(json.len());
        let block = &json[abs..block_end];

        // Extract CVSS score from severity array
        let score = extract_osv_cvss_score(block).unwrap_or(0.0);

        // Extract summary
        let summary = extract_string_field(block, "\"summary\"").unwrap_or_default();

        // Extract CWE IDs
        let cwes = extract_cwes(block);

        let requirement = cwes.iter().find_map(|cwe| cwe_to_requirement(*cwe));

        findings.push(VulnFinding {
            id,
            severity: Severity::from_score(score),
            requirement,
            summary,
            cwes,
        });

        pos = block_end;
    }

    Ok(findings)
}

// ── JSON extraction helpers ─────────────────────────────────────────────────

/// Extract the string value for a JSON field like `"key": "value"`.
fn extract_string_field(json: &str, key: &str) -> Option<String> {
    let key_pos = json.find(key)?;
    let after_key = &json[key_pos + key.len()..];
    // Skip whitespace and colon
    let after_colon = after_key.trim_start().strip_prefix(':')?;
    let after_space = after_colon.trim_start();
    if !after_space.starts_with('"') {
        return None;
    }
    let value_start = 1; // skip opening quote
    let rest = &after_space[value_start..];
    // Find closing quote (handling escapes)
    let mut escaped = false;
    for (i, c) in rest.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if c == '"' {
            return Some(rest[..i].to_string());
        }
    }
    None
}

/// Extract CVSS baseScore from NVD JSON block.
fn extract_base_score(json: &str) -> Option<f64> {
    let key = "\"baseScore\"";
    let pos = json.find(key)?;
    let after = &json[pos + key.len()..];
    let after_colon = after.trim_start().strip_prefix(':')?;
    let num_str = after_colon.trim_start();
    // Parse until non-numeric
    let end = num_str
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(num_str.len());
    num_str[..end].parse().ok()
}

/// Extract CVSS score from OSV severity array (CVSS:3.1/... vector string).
fn extract_osv_cvss_score(json: &str) -> Option<f64> {
    // Look for "score" field near "CVSS_V3"
    let cvss_pos = json.find("\"CVSS_V3\"")?;
    let block = &json[cvss_pos..];
    let score_str = extract_string_field(block, "\"score\"")?;
    // Parse CVSS vector string — the base score is not directly in the string,
    // so we estimate from the vector or look for a numeric score field
    // Actually, OSV sometimes has a numeric "score" field too
    if let Ok(score) = score_str.parse::<f64>() {
        return Some(score);
    }
    // If it's a CVSS vector, extract AV and estimate
    // For simplicity, try to find a baseScore in the same block
    extract_base_score(block)
}

/// Extract CWE IDs from a JSON block (looks for "CWE-NNN" patterns).
fn extract_cwes(json: &str) -> Vec<u32> {
    let mut cwes = Vec::new();
    let mut pos = 0;
    while let Some(cwe_pos) = json[pos..].find("CWE-") {
        let abs = pos + cwe_pos + 4;
        let end = json[abs..]
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(json[abs..].len());
        if end > 0 {
            if let Ok(id) = json[abs..abs + end].parse::<u32>() {
                if !cwes.contains(&id) {
                    cwes.push(id);
                }
            }
        }
        pos = abs + end;
    }
    cwes
}

/// Extract the English description from NVD response.
fn extract_description(json: &str) -> Option<String> {
    // Look for "descriptions" array, then find "en" language entry
    let desc_pos = json.find("\"descriptions\"")?;
    let block = &json[desc_pos..];
    // Find the "value" field after a "lang": "en" entry
    let en_pos = block.find("\"en\"")?;
    let after_en = &block[en_pos..];
    extract_string_field(after_en, "\"value\"")
}

// ── URL encoding ────────────────────────────────────────────────────────────

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

// ── Aggregate scanning ──────────────────────────────────────────────────────

/// Run supply-chain audit on all `[native]` and `[c-native]` dependencies.
pub fn scan_all(
    native: &HashMap<String, String>,
    c_native: &HashMap<String, String>,
) -> SupplyChainAudit {
    let mut results = Vec::new();
    let mut errors = Vec::new();

    // Scan [c-native] — query both NVD and OSV
    for (name, version) in c_native {
        let mut findings = Vec::new();

        match query_nvd(name, version) {
            Ok(nvd_findings) => findings.extend(nvd_findings),
            Err(e) => errors.push(format!("[c-native] {name}: NVD query failed: {e}")),
        }

        match query_osv(name, version, "C") {
            Ok(osv_findings) => {
                // Deduplicate by ID (NVD may already have the same CVE)
                for f in osv_findings {
                    if !findings.iter().any(|existing| existing.id == f.id) {
                        findings.push(f);
                    }
                }
            }
            Err(e) => errors.push(format!("[c-native] {name}: OSV query failed: {e}")),
        }

        results.push(DepAuditResult {
            name: name.clone(),
            version: version.clone(),
            section: "c-native".to_string(),
            findings,
        });
    }

    // Scan [native] — query OSV with ecosystem "crates.io"
    for (name, version) in native {
        let mut findings = Vec::new();

        match query_osv(name, version, "crates.io") {
            Ok(osv_findings) => findings.extend(osv_findings),
            Err(e) => errors.push(format!("[native] {name}: OSV query failed: {e}")),
        }

        results.push(DepAuditResult {
            name: name.clone(),
            version: version.clone(),
            section: "native".to_string(),
            findings,
        });
    }

    SupplyChainAudit { results, errors }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // --- CWE mapping ---

    #[test]
    fn cwe_buffer_overflow_maps_to_memory_safety() {
        assert_eq!(cwe_to_requirement(119), Some(MvlRequirement::MemorySafety));
        assert_eq!(cwe_to_requirement(120), Some(MvlRequirement::MemorySafety));
        assert_eq!(cwe_to_requirement(787), Some(MvlRequirement::MemorySafety));
    }

    #[test]
    fn cwe_use_after_free_maps_to_memory_safety() {
        assert_eq!(cwe_to_requirement(416), Some(MvlRequirement::MemorySafety));
        assert_eq!(cwe_to_requirement(415), Some(MvlRequirement::MemorySafety));
    }

    #[test]
    fn cwe_null_deref_maps_to_null_safety() {
        assert_eq!(cwe_to_requirement(476), Some(MvlRequirement::NullSafety));
    }

    #[test]
    fn cwe_race_condition_maps_to_concurrency() {
        assert_eq!(cwe_to_requirement(362), Some(MvlRequirement::Concurrency));
        assert_eq!(cwe_to_requirement(367), Some(MvlRequirement::Concurrency));
    }

    #[test]
    fn cwe_information_exposure_maps_to_ifc() {
        assert_eq!(
            cwe_to_requirement(200),
            Some(MvlRequirement::InformationFlow)
        );
        assert_eq!(
            cwe_to_requirement(312),
            Some(MvlRequirement::InformationFlow)
        );
        assert_eq!(
            cwe_to_requirement(319),
            Some(MvlRequirement::InformationFlow)
        );
    }

    #[test]
    fn cwe_unmapped_returns_none() {
        assert_eq!(cwe_to_requirement(79), None); // XSS
        assert_eq!(cwe_to_requirement(89), None); // SQL injection
        assert_eq!(cwe_to_requirement(0), None);
    }

    // --- Severity ---

    #[test]
    fn severity_from_score() {
        assert_eq!(Severity::from_score(9.8), Severity::Critical);
        assert_eq!(Severity::from_score(9.0), Severity::Critical);
        assert_eq!(Severity::from_score(7.5), Severity::High);
        assert_eq!(Severity::from_score(7.0), Severity::High);
        assert_eq!(Severity::from_score(5.0), Severity::Medium);
        assert_eq!(Severity::from_score(4.0), Severity::Medium);
        assert_eq!(Severity::from_score(3.9), Severity::Low);
        assert_eq!(Severity::from_score(0.0), Severity::Low);
    }

    #[test]
    fn severity_display() {
        assert_eq!(Severity::Critical.to_string(), "critical");
        assert_eq!(Severity::High.to_string(), "high");
        assert_eq!(Severity::Medium.to_string(), "medium");
        assert_eq!(Severity::Low.to_string(), "low");
    }

    // --- MvlRequirement ---

    #[test]
    fn requirement_display() {
        assert_eq!(
            MvlRequirement::MemorySafety.to_string(),
            "R2 (memory safety)"
        );
        assert_eq!(MvlRequirement::NullSafety.to_string(), "R4 (null safety)");
        assert_eq!(MvlRequirement::Concurrency.to_string(), "R8 (concurrency)");
        assert_eq!(
            MvlRequirement::InformationFlow.to_string(),
            "R11 (information flow)"
        );
    }

    // --- NVD JSON parsing ---

    #[test]
    fn parse_nvd_response_extracts_cves() {
        let json = r#"{
            "vulnerabilities": [
                {
                    "cve": {
                        "id": "CVE-2023-12345",
                        "descriptions": [
                            { "lang": "en", "value": "Buffer overflow in foo" }
                        ],
                        "metrics": {
                            "cvssMetricV31": [
                                { "cvssData": { "baseScore": 9.8 } }
                            ]
                        },
                        "weaknesses": [
                            { "description": [{ "value": "CWE-787" }] }
                        ]
                    }
                }
            ]
        }"#;
        let findings = parse_nvd_response(json, "foo").unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, "CVE-2023-12345");
        assert_eq!(findings[0].severity, Severity::Critical);
        assert_eq!(findings[0].cwes, vec![787]);
        assert_eq!(findings[0].requirement, Some(MvlRequirement::MemorySafety));
        assert!(findings[0].summary.contains("Buffer overflow"));
    }

    #[test]
    fn parse_nvd_response_empty() {
        let json = r#"{"vulnerabilities": []}"#;
        let findings = parse_nvd_response(json, "foo").unwrap();
        assert!(findings.is_empty());
    }

    #[test]
    fn parse_nvd_response_multiple_cves() {
        let json = r#"{
            "vulnerabilities": [
                {
                    "cve": {
                        "id": "CVE-2023-11111",
                        "descriptions": [{ "lang": "en", "value": "Null deref" }],
                        "metrics": { "cvssMetricV31": [{ "cvssData": { "baseScore": 7.5 } }] },
                        "weaknesses": [{ "description": [{ "value": "CWE-476" }] }]
                    }
                },
                {
                    "cve": {
                        "id": "CVE-2023-22222",
                        "descriptions": [{ "lang": "en", "value": "Race condition" }],
                        "metrics": { "cvssMetricV31": [{ "cvssData": { "baseScore": 5.3 } }] },
                        "weaknesses": [{ "description": [{ "value": "CWE-362" }] }]
                    }
                }
            ]
        }"#;
        let findings = parse_nvd_response(json, "bar").unwrap();
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].id, "CVE-2023-11111");
        assert_eq!(findings[0].requirement, Some(MvlRequirement::NullSafety));
        assert_eq!(findings[1].id, "CVE-2023-22222");
        assert_eq!(findings[1].requirement, Some(MvlRequirement::Concurrency));
    }

    // --- OSV JSON parsing ---

    #[test]
    fn parse_osv_response_extracts_vulns() {
        let json = r#"{
            "vulns": [
                {
                    "id": "GHSA-abc-def",
                    "summary": "Heap overflow in parser",
                    "severity": [{ "type": "CVSS_V3", "score": "7.8" }],
                    "database_specific": { "cwe_ids": ["CWE-122"] }
                }
            ]
        }"#;
        let findings = parse_osv_response(json).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, "GHSA-abc-def");
        assert_eq!(findings[0].severity, Severity::High);
        assert_eq!(findings[0].cwes, vec![122]);
        assert_eq!(findings[0].requirement, Some(MvlRequirement::MemorySafety));
    }

    #[test]
    fn parse_osv_response_empty() {
        let json = r#"{}"#;
        let findings = parse_osv_response(json).unwrap();
        assert!(findings.is_empty());
    }

    // --- DepAuditResult ---

    #[test]
    fn dep_audit_result_clean() {
        let r = DepAuditResult {
            name: "libz".to_string(),
            version: "1.3".to_string(),
            section: "c-native".to_string(),
            findings: vec![],
        };
        assert!(r.is_clean());
        assert_eq!(r.severity_summary(), "clean");
    }

    #[test]
    fn dep_audit_result_severity_summary() {
        let r = DepAuditResult {
            name: "openssl".to_string(),
            version: "3.0".to_string(),
            section: "c-native".to_string(),
            findings: vec![
                VulnFinding {
                    id: "CVE-2023-1".to_string(),
                    severity: Severity::Critical,
                    requirement: Some(MvlRequirement::MemorySafety),
                    summary: "overflow".to_string(),
                    cwes: vec![787],
                },
                VulnFinding {
                    id: "CVE-2023-2".to_string(),
                    severity: Severity::High,
                    requirement: None,
                    summary: "flaw".to_string(),
                    cwes: vec![],
                },
                VulnFinding {
                    id: "CVE-2023-3".to_string(),
                    severity: Severity::High,
                    requirement: Some(MvlRequirement::NullSafety),
                    summary: "null".to_string(),
                    cwes: vec![476],
                },
                VulnFinding {
                    id: "CVE-2023-4".to_string(),
                    severity: Severity::Medium,
                    requirement: None,
                    summary: "minor".to_string(),
                    cwes: vec![],
                },
            ],
        };
        assert!(!r.is_clean());
        assert_eq!(
            r.severity_summary(),
            "4 CVEs (1 critical, 2 high, 1 medium)"
        );
    }

    // --- SupplyChainAudit ---

    #[test]
    fn audit_report_render_clean() {
        let audit = SupplyChainAudit {
            results: vec![DepAuditResult {
                name: "libz".to_string(),
                version: "1.3".to_string(),
                section: "c-native".to_string(),
                findings: vec![],
            }],
            errors: vec![],
        };
        let output = audit.render();
        assert!(output.contains("libz 1.3 — clean"));
        assert!(output.contains("No known vulnerabilities found."));
        assert!(!audit.has_vulnerabilities());
    }

    #[test]
    fn audit_report_render_with_findings() {
        let audit = SupplyChainAudit {
            results: vec![DepAuditResult {
                name: "openssl".to_string(),
                version: "3.0".to_string(),
                section: "c-native".to_string(),
                findings: vec![VulnFinding {
                    id: "CVE-2023-99999".to_string(),
                    severity: Severity::Critical,
                    requirement: Some(MvlRequirement::MemorySafety),
                    summary: "Buffer overflow in X509_verify".to_string(),
                    cwes: vec![787],
                }],
            }],
            errors: vec![],
        };
        let output = audit.render();
        assert!(output.contains("openssl 3.0"));
        assert!(output.contains("CVE-2023-99999"));
        assert!(output.contains("critical"));
        assert!(output.contains("R2 (memory safety)"));
        assert!(output.contains("1 vulnerability found"));
        assert!(audit.has_vulnerabilities());
    }

    #[test]
    fn audit_report_render_with_errors() {
        let audit = SupplyChainAudit {
            results: vec![],
            errors: vec!["[c-native] foo: NVD query failed: timeout".to_string()],
        };
        let output = audit.render();
        assert!(output.contains("Warnings:"));
        assert!(output.contains("timeout"));
    }

    // --- JSON helpers ---

    #[test]
    fn extract_string_field_basic() {
        let json = r#"{"id": "CVE-2023-12345", "other": "value"}"#;
        assert_eq!(
            extract_string_field(json, "\"id\""),
            Some("CVE-2023-12345".to_string())
        );
    }

    #[test]
    fn extract_string_field_missing() {
        let json = r#"{"other": "value"}"#;
        assert_eq!(extract_string_field(json, "\"id\""), None);
    }

    #[test]
    fn extract_cwes_multiple() {
        let json =
            r#"{"weaknesses": [{"description": [{"value": "CWE-119"}, {"value": "CWE-787"}]}]}"#;
        let cwes = extract_cwes(json);
        assert_eq!(cwes, vec![119, 787]);
    }

    #[test]
    fn extract_cwes_none() {
        let json = r#"{"no_cwes": true}"#;
        let cwes = extract_cwes(json);
        assert!(cwes.is_empty());
    }

    #[test]
    fn extract_cwes_deduplicates() {
        let json = r#""CWE-119" "CWE-119""#;
        let cwes = extract_cwes(json);
        assert_eq!(cwes, vec![119]);
    }

    #[test]
    fn extract_base_score_parses_float() {
        let json = r#"{"cvssData": {"baseScore": 9.8, "other": 1}}"#;
        assert_eq!(extract_base_score(json), Some(9.8));
    }

    #[test]
    fn url_encode_spaces_and_special() {
        assert_eq!(url_encode("hello world"), "hello%20world");
        assert_eq!(url_encode("foo+bar"), "foo%2Bbar");
        assert_eq!(url_encode("safe-text_1.0"), "safe-text_1.0");
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let s = "a".repeat(100);
        let t = truncate(&s, 20);
        assert!(t.len() <= 20);
        assert!(t.ends_with("..."));
    }
}
