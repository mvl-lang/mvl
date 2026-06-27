// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Per-section parsers for `mvl.toml`: `[dependencies]`,
//! `[dependency-policy]`, `[security]`, `[native]`, `[c-native]`,
//! `[license-policy]`.
//!
//! Split out of `manifest.rs` (#1562).  All parsers are `pub(super)` so
//! `Manifest::parse` in the parent module can dispatch to them.

use std::collections::HashMap;

use super::toml::TomlValue;
use super::{
    CNativeSpec, DepSpec, DependencyPolicy, LicensePolicy, LicensePolicyMode, ManifestError,
    SecurityPolicy,
};

pub(super) fn parse_dependencies(
    value: Option<&TomlValue>,
) -> Result<HashMap<String, DepSpec>, ManifestError> {
    let mut deps = HashMap::new();
    let tbl = match value {
        None => return Ok(deps),
        Some(v) => v.as_table().ok_or_else(|| {
            ManifestError::ParseError("[dependencies] must be a table".to_string())
        })?,
    };
    for (name, val) in tbl {
        let spec = match val {
            TomlValue::String(s) => DepSpec::Version(s.clone()),
            TomlValue::Table(t) => {
                let git = t
                    .get("git")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ManifestError::ParseError(format!("dep '{name}': missing 'git'"))
                    })?
                    .to_string();
                let tag = t
                    .get("tag")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ManifestError::ParseError(format!("dep '{name}': missing 'tag'"))
                    })?
                    .to_string();
                let rationale = t
                    .get("rationale")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let exclude = t
                    .get("exclude")
                    .and_then(|v| v.as_string_array())
                    .map(|a| a.to_vec())
                    .unwrap_or_default();
                DepSpec::Git {
                    git,
                    tag,
                    rationale,
                    exclude,
                }
            }
            _ => {
                return Err(ManifestError::ParseError(format!(
                    "dep '{name}': invalid dependency spec (expected string or inline table)"
                )));
            }
        };
        deps.insert(name.clone(), spec);
    }
    Ok(deps)
}

pub(super) fn parse_dependency_policy(
    value: Option<&TomlValue>,
) -> Result<DependencyPolicy, ManifestError> {
    let mut policy = DependencyPolicy::default();
    let tbl = match value {
        None => return Ok(policy),
        Some(v) => v.as_table().ok_or_else(|| {
            ManifestError::ParseError("[dependency-policy] must be a table".to_string())
        })?,
    };
    if let Some(v) = tbl.get("complexity-threshold") {
        policy.complexity_threshold = v.as_integer().ok_or_else(|| {
            ManifestError::ParseError(
                "dependency-policy: complexity-threshold must be an integer".to_string(),
            )
        })? as u64;
    }
    if let Some(v) = tbl.get("rationale-required") {
        policy.rationale_required = v.as_bool().ok_or_else(|| {
            ManifestError::ParseError(
                "dependency-policy: rationale-required must be a boolean".to_string(),
            )
        })?;
    }
    Ok(policy)
}

pub(super) fn parse_security_policy(
    value: Option<&TomlValue>,
) -> Result<SecurityPolicy, ManifestError> {
    let mut policy = SecurityPolicy::default();
    let tbl = match value {
        None => return Ok(policy),
        Some(v) => v
            .as_table()
            .ok_or_else(|| ManifestError::ParseError("[security] must be a table".to_string()))?,
    };
    if let Some(v) = tbl.get("min-age-days") {
        let n = v.as_integer().ok_or_else(|| {
            ManifestError::ParseError("security: min-age-days must be an integer".to_string())
        })?;
        if n < 0 {
            return Err(ManifestError::ParseError(
                "security: min-age-days must be >= 0".to_string(),
            ));
        }
        policy.min_age_days = n as u64;
    }
    Ok(policy)
}

pub(super) fn parse_native(
    value: Option<&TomlValue>,
    section: &str,
) -> Result<HashMap<String, String>, ManifestError> {
    let mut native = HashMap::new();
    let tbl = match value {
        None => return Ok(native),
        Some(v) => v
            .as_table()
            .ok_or_else(|| ManifestError::ParseError(format!("[{section}] must be a table")))?,
    };
    for (name, val) in tbl {
        // Accept either a plain string ("0.31") or a table with a `version` key
        // ({ version = "0.31", features = [...] }).
        let version = if let Some(s) = val.as_str() {
            s.to_string()
        } else if let Some(t) = val.as_table() {
            t.get("version")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ManifestError::ParseError(format!(
                        "{section} dep '{name}' table must have a 'version' string"
                    ))
                })?
                .to_string()
        } else {
            return Err(ManifestError::ParseError(format!(
                "{section} dep '{name}' must be a string or table with 'version'"
            )));
        };
        native.insert(name.clone(), version);
    }
    Ok(native)
}

/// Parse `[c-native]` section into `CNativeSpec` entries (#635).
///
/// Accepts bare strings (`libz = "1.3"`) or inline tables with an optional
/// `license` field (`libz = { version = "1.3", license = "Zlib" }`).
pub(super) fn parse_c_native_section(
    value: Option<&TomlValue>,
) -> Result<HashMap<String, CNativeSpec>, ManifestError> {
    let mut deps = HashMap::new();
    let tbl = match value {
        None => return Ok(deps),
        Some(v) => v
            .as_table()
            .ok_or_else(|| ManifestError::ParseError("[c-native] must be a table".to_string()))?,
    };
    for (name, val) in tbl {
        let spec = if let Some(s) = val.as_str() {
            CNativeSpec {
                version: s.to_string(),
                license: None,
            }
        } else if let Some(t) = val.as_table() {
            let version = t
                .get("version")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ManifestError::ParseError(format!(
                        "c-native dep '{name}' table must have a 'version' string"
                    ))
                })?
                .to_string();
            let license = t
                .get("license")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            CNativeSpec { version, license }
        } else {
            return Err(ManifestError::ParseError(format!(
                "c-native dep '{name}' must be a string or table with 'version'"
            )));
        };
        deps.insert(name.clone(), spec);
    }
    Ok(deps)
}

/// Parse `[license-policy]` section (#635).
pub(super) fn parse_license_policy(
    value: Option<&TomlValue>,
) -> Result<LicensePolicy, ManifestError> {
    let mut policy = LicensePolicy::default();
    let tbl = match value {
        None => return Ok(policy),
        Some(v) => v.as_table().ok_or_else(|| {
            ManifestError::ParseError("[license-policy] must be a table".to_string())
        })?,
    };
    if let Some(v) = tbl.get("mode") {
        let mode_str = v.as_str().ok_or_else(|| {
            ManifestError::ParseError("license-policy: mode must be a string".to_string())
        })?;
        policy.mode = match mode_str {
            "permissive" => LicensePolicyMode::Permissive,
            "copyleft-ok" => LicensePolicyMode::CopyleftOk,
            "any" => LicensePolicyMode::Any,
            "custom" => LicensePolicyMode::Custom,
            other => {
                return Err(ManifestError::ParseError(format!(
                    "license-policy: unknown mode '{other}'; expected permissive, copyleft-ok, any, or custom"
                )));
            }
        };
    }
    if let Some(v) = tbl.get("allow") {
        policy.allow = v
            .as_string_array()
            .ok_or_else(|| {
                ManifestError::ParseError(
                    "license-policy: allow must be an array of strings".to_string(),
                )
            })?
            .to_vec();
    }
    if let Some(v) = tbl.get("deny") {
        policy.deny = v
            .as_string_array()
            .ok_or_else(|| {
                ManifestError::ParseError(
                    "license-policy: deny must be an array of strings".to_string(),
                )
            })?
            .to_vec();
    }
    Ok(policy)
}
