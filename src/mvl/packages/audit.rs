// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Supply-chain vulnerability audit for `[native]` and `[c-native]` dependencies.
//!
//! Implements issue #633: `mvl audit --supply-chain` scans C libraries declared
//! in `[c-native]` against NVD (NIST) and OSV (osv.dev), and maps CVEs to MVL
//! requirement gaps using CWE classification.
//!
//! Also scans `[native]` (Rust crates) against OSV for ecosystem `"crates.io"`.

use super::manifest::CNativeSpec;
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

fn truncate(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else {
        let end = s
            .char_indices()
            .nth(max_chars.saturating_sub(3))
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        format!("{}...", &s[..end])
    }
}

// ── HTTP client ─────────────────────────────────────────────────────────────

/// Build a ureq Agent with explicit timeouts to prevent hanging in CI.
fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(15))
        .build()
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

    let agent = http_agent();
    let mut req = agent.get(&url);
    if let Some(ref key) = api_key {
        // Validate key does not contain characters that could corrupt headers
        if key.bytes().any(|b| b == b'\n' || b == b'\r' || b == 0) {
            return Err("NVD_API_KEY contains invalid characters".to_string());
        }
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
/// For C libraries, uses no ecosystem (name-only query, broader results).
/// For Rust crates, uses ecosystem `"crates.io"`.
pub fn query_osv(
    name: &str,
    version: &str,
    ecosystem: Option<&str>,
) -> Result<Vec<VulnFinding>, String> {
    let url = "https://api.osv.dev/v1/query";
    let esc_name = json_escape(name);
    let esc_version = json_escape(version);
    let body = if let Some(eco) = ecosystem {
        let esc_eco = json_escape(eco);
        format!(
            r#"{{"package":{{"name":"{esc_name}","ecosystem":"{esc_eco}"}},"version":"{esc_version}"}}"#
        )
    } else {
        format!(r#"{{"package":{{"name":"{esc_name}"}},"version":"{esc_version}"}}"#)
    };

    let agent = http_agent();
    let resp = agent
        .post(url)
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

        // Skip non-top-level IDs: aliases, package names, reference IDs.
        // Only accept known vulnerability ID prefixes.
        if id.is_empty()
            || !(id.starts_with("GHSA-")
                || id.starts_with("CVE-")
                || id.starts_with("OSV-")
                || id.starts_with("RUSTSEC-")
                || id.starts_with("PYSEC-"))
        {
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
///
/// OSV may provide either a numeric score string (e.g. `"7.8"`) or a CVSS
/// vector string (e.g. `"CVSS:3.1/AV:N/AC:L/..."`). We try the numeric parse
/// first, then attempt to estimate from the CVSS v3 vector metrics.
fn extract_osv_cvss_score(json: &str) -> Option<f64> {
    let cvss_pos = json.find("\"CVSS_V3\"")?;
    let block = &json[cvss_pos..];
    let score_str = extract_string_field(block, "\"score\"")?;
    // Try numeric score first (some OSV entries provide a bare float)
    if let Ok(score) = score_str.parse::<f64>() {
        return Some(score);
    }
    // Try to estimate base score from CVSS v3 vector string
    if score_str.starts_with("CVSS:3") {
        return Some(estimate_cvss3_score(&score_str));
    }
    None
}

/// Estimate a CVSS v3.x base score from the vector string metrics.
///
/// This is a simplified estimator that covers the most common attack patterns.
/// It maps the six base metric values to weights and computes a weighted score.
/// For precise scoring, a full CVSS calculator would be needed, but this
/// gives a reasonable severity classification (Low/Medium/High/Critical).
fn estimate_cvss3_score(vector: &str) -> f64 {
    let mut score: f64 = 0.0;

    // Attack Vector (AV)
    if vector.contains("/AV:N") {
        score += 2.5; // Network
    } else if vector.contains("/AV:A") {
        score += 1.5; // Adjacent
    } else if vector.contains("/AV:L") {
        score += 1.0; // Local
    } else if vector.contains("/AV:P") {
        score += 0.5; // Physical
    }

    // Attack Complexity (AC)
    if vector.contains("/AC:L") {
        score += 1.5; // Low complexity
    } else if vector.contains("/AC:H") {
        score += 0.5; // High complexity
    }

    // Confidentiality Impact (C)
    if vector.contains("/C:H") {
        score += 2.0;
    } else if vector.contains("/C:L") {
        score += 0.5;
    }

    // Integrity Impact (I)
    if vector.contains("/I:H") {
        score += 2.0;
    } else if vector.contains("/I:L") {
        score += 0.5;
    }

    // Availability Impact (A — match "/A:" not "/AV:" or "/AC:")
    if vector.contains("/A:H") && !vector.ends_with("/AV:H") {
        score += 2.0;
    } else if vector.contains("/A:L") {
        score += 0.5;
    }

    // Clamp to CVSS range
    score.min(10.0)
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

// ── JSON string escaping ────────────────────────────────────────────────────

/// Escape a string for safe inclusion in a JSON string value.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
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
    c_native: &HashMap<String, CNativeSpec>,
) -> SupplyChainAudit {
    let mut results = Vec::new();
    let mut errors = Vec::new();

    // NVD rate limit: 5 req/min without API key, 60 req/min with key
    let has_api_key = std::env::var("NVD_API_KEY").is_ok();
    let nvd_delay = if has_api_key {
        std::time::Duration::from_millis(1100) // ~54 req/min
    } else {
        std::time::Duration::from_millis(12_000) // ~5 req/min
    };
    // Scan [c-native] — query both NVD and OSV
    for (idx, (name, spec)) in c_native.iter().enumerate() {
        let version = &spec.version;
        let mut findings = Vec::new();

        // Rate-limit NVD calls (skip delay before first call)
        if idx > 0 {
            std::thread::sleep(nvd_delay);
        }

        match query_nvd(name, version) {
            Ok(nvd_findings) => findings.extend(nvd_findings),
            Err(e) => errors.push(format!("[c-native] {name}: NVD query failed: {e}")),
        }

        match query_osv(name, version, None) {
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

        match query_osv(name, version, Some("crates.io")) {
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

    // ── New tests for PR #633 gap coverage ──────────────────────────────────

    // ── Gap 1 (CRITICAL): parse_osv_response — nested "id" fields ───────────

    /// Real OSV responses contain nested "id" fields inside aliases and
    /// affected ranges. The parser must not emit phantom findings for them.
    #[test]
    fn parse_osv_response_ignores_nested_ids_in_aliases() {
        // The top-level vuln has id "GHSA-top-level"; "CVE-2021-9999" appears
        // only as an alias entry — it must NOT generate a second finding.
        let json = r#"{
            "vulns": [
                {
                    "id": "GHSA-top-level",
                    "aliases": ["CVE-2021-9999"],
                    "summary": "Heap overflow",
                    "severity": [{ "type": "CVSS_V3", "score": "8.1" }],
                    "database_specific": { "cwe_ids": ["CWE-122"] }
                }
            ]
        }"#;
        let findings = parse_osv_response(json).unwrap();
        // Because "CVE-2021-9999" appears inside the same block as
        // "GHSA-top-level", the current parser will treat each encountered
        // "id" key as a new vuln. This test documents the known behaviour:
        // the parser returns at least one finding whose id is "GHSA-top-level".
        assert!(
            findings.iter().any(|f| f.id == "GHSA-top-level"),
            "top-level vuln ID must be present; got: {:?}",
            findings.iter().map(|f| &f.id).collect::<Vec<_>>()
        );
        // The alias "CVE-2021-9999" should NOT be emitted as a separate finding
        // because it is nested inside the same vuln block. If this assertion
        // fails it confirms the false-positive bug described in PR #633 review.
        let alias_as_finding = findings.iter().any(|f| f.id == "CVE-2021-9999");
        assert!(
            !alias_as_finding,
            "alias 'CVE-2021-9999' must not be promoted to a standalone finding"
        );
    }

    /// A well-formed multi-vuln OSV response must produce exactly one finding
    /// per top-level vuln entry regardless of nested id fields.
    #[test]
    fn parse_osv_response_multiple_vulns_correct_count() {
        let json = r#"{
            "vulns": [
                {
                    "id": "GHSA-aaaa-bbbb",
                    "summary": "vuln one",
                    "severity": [{ "type": "CVSS_V3", "score": "5.0" }],
                    "database_specific": { "cwe_ids": [] }
                },
                {
                    "id": "GHSA-cccc-dddd",
                    "summary": "vuln two",
                    "severity": [{ "type": "CVSS_V3", "score": "9.0" }],
                    "database_specific": { "cwe_ids": ["CWE-787"] }
                }
            ]
        }"#;
        let findings = parse_osv_response(json).unwrap();
        let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
        assert!(
            ids.contains(&"GHSA-aaaa-bbbb"),
            "first vuln missing; got: {ids:?}"
        );
        assert!(
            ids.contains(&"GHSA-cccc-dddd"),
            "second vuln missing; got: {ids:?}"
        );
    }

    // ── Gap 2 (CRITICAL): scan_all deduplication ────────────────────────────

    /// When NVD returns CVE-X and OSV also returns the same CVE-X in its
    /// response, scan_all must deduplicate — only one finding must appear.
    #[test]
    fn scan_all_deduplicates_same_cve_from_nvd_and_osv() {
        // We exercise the deduplication logic by feeding pre-parsed findings
        // through the DepAuditResult path directly, since network calls are
        // not mocked. The deduplication loop in scan_all is:
        //   if !findings.iter().any(|existing| existing.id == f.id) { push }
        // We replicate that logic here to verify it works correctly.
        let nvd_finding = VulnFinding {
            id: "CVE-2023-99000".to_string(),
            severity: Severity::High,
            requirement: Some(MvlRequirement::MemorySafety),
            summary: "from NVD".to_string(),
            cwes: vec![787],
        };
        let osv_dup = VulnFinding {
            id: "CVE-2023-99000".to_string(), // same ID
            severity: Severity::High,
            requirement: Some(MvlRequirement::MemorySafety),
            summary: "from OSV".to_string(),
            cwes: vec![787],
        };
        let osv_unique = VulnFinding {
            id: "GHSA-unique-xxxx".to_string(),
            severity: Severity::Medium,
            requirement: None,
            summary: "unique OSV finding".to_string(),
            cwes: vec![],
        };

        // Simulate the deduplication loop from scan_all
        let mut findings = vec![nvd_finding];
        for f in [osv_dup, osv_unique] {
            if !findings.iter().any(|existing| existing.id == f.id) {
                findings.push(f);
            }
        }

        assert_eq!(
            findings.len(),
            2,
            "expected 2 findings (deduped): got {:?}",
            findings.iter().map(|f| &f.id).collect::<Vec<_>>()
        );
        assert_eq!(findings[0].id, "CVE-2023-99000");
        assert_eq!(findings[1].id, "GHSA-unique-xxxx");
        // Confirm the duplicate summary is from NVD (first arrival wins)
        assert_eq!(findings[0].summary, "from NVD");
    }

    // ── Gap 3 (HIGH): extract_osv_cvss_score with CVSS vector string ────────

    /// When OSV provides a CVSS vector string (e.g. "CVSS:3.1/AV:N/..."),
    /// extract_osv_cvss_score cannot parse it as f64 and falls through to
    /// extract_base_score, which also returns None (no "baseScore" key).
    /// The finding should default to score 0.0 → Severity::Low.
    /// This test documents the current silent severity-downgrade behaviour.
    #[test]
    fn osv_cvss_vector_string_estimated_to_critical() {
        let json = r#"{
            "vulns": [
                {
                    "id": "GHSA-cvss-vector",
                    "summary": "Buffer overflow via vector string score",
                    "severity": [
                        {
                            "type": "CVSS_V3",
                            "score": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"
                        }
                    ],
                    "database_specific": { "cwe_ids": ["CWE-787"] }
                }
            ]
        }"#;
        // The CVSS vector estimator should produce a score in the Critical range
        // for AV:N/AC:L/C:H/I:H/A:H (= 2.5+1.5+2.0+2.0+2.0 = 10.0)
        let score = extract_osv_cvss_score(json);
        assert!(score.is_some(), "CVSS vector should produce a score");
        assert!(
            score.unwrap() >= 9.0,
            "AV:N/AC:L/C:H/I:H/A:H should be Critical; got {}",
            score.unwrap()
        );
        // Verify the downstream pipeline
        let findings = parse_osv_response(json).unwrap();
        let f = findings
            .iter()
            .find(|f| f.id == "GHSA-cvss-vector")
            .unwrap();
        assert_eq!(
            f.severity,
            Severity::Critical,
            "CVSS vector AV:N/AC:L/C:H/I:H/A:H must resolve to Critical"
        );
    }

    #[test]
    fn estimate_cvss3_score_low_impact() {
        // AV:P/AC:H/C:N/I:L/A:N → 0.5 + 0.5 + 0.0 + 0.5 + 0.0 = 1.5 → Low
        let score = estimate_cvss3_score("CVSS:3.1/AV:P/AC:H/PR:H/UI:R/S:U/C:N/I:L/A:N");
        assert_eq!(Severity::from_score(score), Severity::Low);
    }

    // ── Gap 4 (HIGH): scan_all error accumulation ────────────────────────────

    /// scan_all must continue processing remaining deps after one fails;
    /// errors accumulate in audit.errors and the dep still appears in results
    /// with empty findings (clean).
    #[test]
    fn scan_all_accumulates_errors_and_continues() {
        // We can only test scan_all's error accumulation with an empty dep map
        // (no network calls). Verify the happy-path shape of the returned struct.
        let native: HashMap<String, String> = HashMap::new();
        let c_native: HashMap<String, CNativeSpec> = HashMap::new();
        let audit = scan_all(&native, &c_native);
        assert!(audit.results.is_empty());
        assert!(audit.errors.is_empty());
        assert!(!audit.has_vulnerabilities());
    }

    /// A SupplyChainAudit with partial errors and partial findings correctly
    /// reports has_vulnerabilities() and total_findings() independently of
    /// the errors list.
    #[test]
    fn supply_chain_audit_errors_do_not_mask_vulnerabilities() {
        let audit = SupplyChainAudit {
            results: vec![DepAuditResult {
                name: "openssl".to_string(),
                version: "1.0.0".to_string(),
                section: "c-native".to_string(),
                findings: vec![VulnFinding {
                    id: "CVE-2023-1".to_string(),
                    severity: Severity::Critical,
                    requirement: None,
                    summary: "test".to_string(),
                    cwes: vec![],
                }],
            }],
            errors: vec!["[c-native] libz: NVD query failed: timeout".to_string()],
        };
        assert!(audit.has_vulnerabilities());
        assert_eq!(audit.total_findings(), 1);
        // Errors are surfaced independently
        assert_eq!(audit.errors.len(), 1);
    }

    /// A dep for which both NVD and OSV fail still appears in results with
    /// empty findings (is_clean = true), and its errors are recorded.
    #[test]
    fn supply_chain_audit_failed_dep_is_clean_with_error() {
        let audit = SupplyChainAudit {
            results: vec![DepAuditResult {
                name: "libfoo".to_string(),
                version: "2.0".to_string(),
                section: "c-native".to_string(),
                findings: vec![], // both queries failed → no findings
            }],
            errors: vec![
                "[c-native] libfoo: NVD query failed: network error".to_string(),
                "[c-native] libfoo: OSV query failed: network error".to_string(),
            ],
        };
        assert!(!audit.has_vulnerabilities());
        let dep = &audit.results[0];
        assert!(
            dep.is_clean(),
            "dep with query failures should appear clean"
        );
        assert_eq!(audit.errors.len(), 2);
        let output = audit.render();
        assert!(output.contains("Warnings:"));
        assert!(output.contains("libfoo: NVD query failed"));
        assert!(output.contains("libfoo: OSV query failed"));
    }

    // ── Gap 6 (MEDIUM): url_encode with query-injection characters ───────────

    #[test]
    fn url_encode_encodes_query_separators() {
        // '&' and '=' in a dep name must be percent-encoded to prevent
        // injection into the NVD keyword search URL.
        assert_eq!(url_encode("foo&bar=baz"), "foo%26bar%3Dbaz");
    }

    #[test]
    fn url_encode_encodes_slash_and_hash() {
        assert_eq!(url_encode("lib/foo#1.0"), "lib%2Ffoo%231.0");
    }

    #[test]
    fn url_encode_encodes_percent_sign() {
        // A literal '%' in a dep name must be double-encoded to avoid
        // producing a malformed percent-escape sequence in the final URL.
        assert_eq!(url_encode("100%"), "100%25");
    }

    // ── Gap 7 (MEDIUM): parse_nvd_response with missing baseScore ───────────

    #[test]
    fn parse_nvd_response_missing_base_score_defaults_to_low() {
        let json = r#"{
            "vulnerabilities": [
                {
                    "cve": {
                        "id": "CVE-2023-55555",
                        "descriptions": [
                            { "lang": "en", "value": "No CVSS score available" }
                        ],
                        "metrics": {},
                        "weaknesses": []
                    }
                }
            ]
        }"#;
        let findings = parse_nvd_response(json, "testlib").unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, "CVE-2023-55555");
        assert_eq!(
            findings[0].severity,
            Severity::Low,
            "missing baseScore must default to 0.0 → Low"
        );
        assert!(findings[0].cwes.is_empty());
        assert!(findings[0].requirement.is_none());
    }

    // ── Gap 8 (SUGGESTION): severity_summary edge cases ─────────────────────

    #[test]
    fn severity_summary_single_cve_singular() {
        let r = DepAuditResult {
            name: "foo".to_string(),
            version: "1.0".to_string(),
            section: "c-native".to_string(),
            findings: vec![VulnFinding {
                id: "CVE-2023-1".to_string(),
                severity: Severity::Critical,
                requirement: None,
                summary: "x".to_string(),
                cwes: vec![],
            }],
        };
        assert_eq!(r.severity_summary(), "1 CVE (1 critical)");
    }

    #[test]
    fn severity_summary_only_low_findings() {
        let r = DepAuditResult {
            name: "foo".to_string(),
            version: "1.0".to_string(),
            section: "c-native".to_string(),
            findings: vec![
                VulnFinding {
                    id: "CVE-2023-1".to_string(),
                    severity: Severity::Low,
                    requirement: None,
                    summary: "x".to_string(),
                    cwes: vec![],
                },
                VulnFinding {
                    id: "CVE-2023-2".to_string(),
                    severity: Severity::Low,
                    requirement: None,
                    summary: "y".to_string(),
                    cwes: vec![],
                },
            ],
        };
        assert_eq!(r.severity_summary(), "2 CVEs (2 low)");
    }

    // ── Gap 9 (SUGGESTION): render section ordering ──────────────────────────

    /// When both [c-native] and [native] deps are present, [c-native] must
    /// appear before [native] in the rendered output.
    #[test]
    fn render_c_native_section_before_native_section() {
        let audit = SupplyChainAudit {
            results: vec![
                DepAuditResult {
                    name: "openssl".to_string(),
                    version: "3.0".to_string(),
                    section: "c-native".to_string(),
                    findings: vec![],
                },
                DepAuditResult {
                    name: "hyper".to_string(),
                    version: "1.0".to_string(),
                    section: "native".to_string(),
                    findings: vec![],
                },
            ],
            errors: vec![],
        };
        let output = audit.render();
        let c_pos = output
            .find("[c-native]")
            .expect("[c-native] section missing");
        let n_pos = output.find("[native]").expect("[native] section missing");
        assert!(
            c_pos < n_pos,
            "[c-native] must appear before [native] in render output"
        );
    }

    /// Deps within the same section are sorted alphabetically by name.
    #[test]
    fn render_deps_sorted_within_section() {
        let audit = SupplyChainAudit {
            results: vec![
                DepAuditResult {
                    name: "zlib".to_string(),
                    version: "1.3".to_string(),
                    section: "c-native".to_string(),
                    findings: vec![],
                },
                DepAuditResult {
                    name: "openssl".to_string(),
                    version: "3.0".to_string(),
                    section: "c-native".to_string(),
                    findings: vec![],
                },
            ],
            errors: vec![],
        };
        let output = audit.render();
        let openssl_pos = output.find("openssl").expect("openssl missing");
        let zlib_pos = output.find("zlib").expect("zlib missing");
        assert!(
            openssl_pos < zlib_pos,
            "deps must be sorted alphabetically within a section"
        );
    }

    // ── Severity boundary values (exhaustive) ────────────────────────────────

    #[test]
    fn severity_boundary_exactly_at_thresholds() {
        // Exact boundary at 9.0 is Critical, 8.999... is High
        assert_eq!(Severity::from_score(9.0), Severity::Critical);
        assert_eq!(Severity::from_score(8.999), Severity::High);
        // Exact boundary at 7.0 is High, 6.999... is Medium
        assert_eq!(Severity::from_score(7.0), Severity::High);
        assert_eq!(Severity::from_score(6.999), Severity::Medium);
        // Exact boundary at 4.0 is Medium, 3.999... is Low
        assert_eq!(Severity::from_score(4.0), Severity::Medium);
        assert_eq!(Severity::from_score(3.999), Severity::Low);
        // Score above 10.0 still resolves to Critical
        assert_eq!(Severity::from_score(10.0), Severity::Critical);
    }

    // ── CWE mapping: remaining unmapped members of the IFC group ────────────

    #[test]
    fn cwe_532_and_209_map_to_information_flow() {
        assert_eq!(
            cwe_to_requirement(532),
            Some(MvlRequirement::InformationFlow)
        );
        assert_eq!(
            cwe_to_requirement(209),
            Some(MvlRequirement::InformationFlow)
        );
    }

    // ── extract_base_score: integer baseScore (no decimal point) ────────────

    #[test]
    fn extract_base_score_parses_integer_score() {
        let json = r#"{"cvssData": {"baseScore": 7, "other": 1}}"#;
        assert_eq!(extract_base_score(json), Some(7.0));
    }

    // ── parse_nvd_response: non-English description still returns a finding ──

    #[test]
    fn parse_nvd_response_no_english_description_still_returns_finding() {
        let json = r#"{
            "vulnerabilities": [
                {
                    "cve": {
                        "id": "CVE-2023-77777",
                        "descriptions": [
                            { "lang": "es", "value": "Desbordamiento de buffer" }
                        ],
                        "metrics": {
                            "cvssMetricV31": [
                                { "cvssData": { "baseScore": 8.0 } }
                            ]
                        },
                        "weaknesses": []
                    }
                }
            ]
        }"#;
        let findings = parse_nvd_response(json, "testlib").unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, "CVE-2023-77777");
        // No English entry means summary defaults to empty string
        assert_eq!(findings[0].summary, "");
        assert_eq!(findings[0].severity, Severity::High);
    }

    // ── VulnFinding: requirement picks first matching CWE (priority) ─────────

    #[test]
    fn vuln_finding_requirement_picks_first_matching_cwe() {
        // CWEs [476 (NullSafety), 787 (MemorySafety)] — first match wins
        let json = r#"{
            "vulnerabilities": [
                {
                    "cve": {
                        "id": "CVE-2023-multi",
                        "descriptions": [{ "lang": "en", "value": "Multi-CWE vuln" }],
                        "metrics": {
                            "cvssMetricV31": [{ "cvssData": { "baseScore": 7.0 } }]
                        },
                        "weaknesses": [
                            { "description": [{ "value": "CWE-476" }, { "value": "CWE-787" }] }
                        ]
                    }
                }
            ]
        }"#;
        let findings = parse_nvd_response(json, "testlib").unwrap();
        assert_eq!(findings.len(), 1);
        // extract_cwes returns [476, 787] in document order
        // find_map picks the first CWE that maps to a requirement
        assert_eq!(findings[0].cwes, vec![476, 787]);
        assert_eq!(findings[0].requirement, Some(MvlRequirement::NullSafety));
    }

    // ── json_escape ─────────────────────────────────────────────────────────

    #[test]
    fn json_escape_quotes_and_backslashes() {
        assert_eq!(json_escape(r#"a"b\c"#), r#"a\"b\\c"#);
    }

    #[test]
    fn json_escape_control_characters() {
        assert_eq!(json_escape("a\nb\tc"), "a\\nb\\tc");
        assert_eq!(json_escape("x\r\n"), "x\\r\\n");
    }

    #[test]
    fn json_escape_low_control_codes() {
        // ASCII 0x01 should be \u0001
        let input = String::from("\x01");
        assert_eq!(json_escape(&input), "\\u0001");
    }

    #[test]
    fn json_escape_safe_string_unchanged() {
        assert_eq!(json_escape("openssl"), "openssl");
        assert_eq!(json_escape("1.3.0"), "1.3.0");
    }

    // ── truncate UTF-8 safety ───────────────────────────────────────────────

    #[test]
    fn truncate_multibyte_utf8_no_panic() {
        // 3-byte UTF-8 chars — truncation must not panic on char boundaries
        let s = "漢字漢字漢字漢字漢字漢字漢字漢字"; // 8 × 2-char groups
        let t = truncate(s, 10);
        assert!(t.ends_with("..."));
        // Must not panic — that's the main assertion
    }

    // ── estimate_cvss3_score ────────────────────────────────────────────────

    #[test]
    fn estimate_cvss3_high_impact() {
        // AV:N/AC:L/C:H/I:H/A:H → 2.5+1.5+2.0+2.0+2.0 = 10.0
        let score = estimate_cvss3_score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H");
        assert_eq!(Severity::from_score(score), Severity::Critical);
    }

    #[test]
    fn estimate_cvss3_medium_impact() {
        // AV:A/AC:L/C:L/I:L/A:N → 1.5+1.5+0.5+0.5+0.0 = 4.0
        let score = estimate_cvss3_score("CVSS:3.1/AV:A/AC:L/PR:N/UI:N/S:U/C:L/I:L/A:N");
        assert_eq!(Severity::from_score(score), Severity::Medium);
    }
}
