use std::fs;
use std::path::Path;
use std::process::Command;

// Note: stdlib source files are no longer embedded in the binary (#1765).
// They ship as a separate `mvl-stdlib-{version}.tar.gz` release artifact and
// are installed to `$XDG_DATA_HOME/mvl/toolchains/{compiler_version}/std/`
// by `make install` (dev) or `mvl self install` (end-user).

// ── Build-metadata helpers ─────────────────────────────────────────────────

/// Return `rustc --version` output (e.g. `"rustc 1.87.0 (17067e9ac 2025-05-09)"`)
/// or an empty string when rustc is not on PATH.
fn rustc_version() -> String {
    Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Return `llvm-config --version` output (e.g. `"18.1.8"`)
/// or an empty string when llvm-config is absent.
fn llvm_version() -> String {
    Command::new("llvm-config")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Return current UTC time as an ISO-8601 string, e.g. `"2026-06-04T14:23:01Z"`.
///
/// Implemented in pure `std` to avoid adding a build-dependency on `chrono`.
fn build_date() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Convert days since 1970-01-01 to (year, month, day).
///
/// Algorithm by Howard Hinnant — https://howardhinnant.github.io/date_algorithms.html
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468_i64;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // day-of-era  [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // year-of-era [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day-of-year [0, 365]
    let mp = (5 * doy + 2) / 153; // month-part  [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // day         [1, 31]
    let mo = if mp < 10 { mp + 3 } else { mp - 9 }; // month       [1, 12]
    let y = if mo <= 2 { y + 1 } else { y };
    (y as i32, mo as u32, d as u32)
}

/// Read the `version = "…"` field from a Cargo.toml file.
fn read_toml_version(path: &Path) -> String {
    let content = fs::read_to_string(path).unwrap_or_default();
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("version") {
            if let Some(v) = line.split('"').nth(1) {
                return v.to_string();
            }
        }
    }
    "unknown".to_string()
}

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();

    // Expose the mvl_runtime crate version so manifest_embed.rs can embed it.
    let runtime_toml = Path::new(&manifest_dir).join("runtime/rust/Cargo.toml");
    println!(
        "cargo:rustc-env=MVL_RUNTIME_VERSION={}",
        read_toml_version(&runtime_toml)
    );
    println!("cargo:rerun-if-changed=runtime/rust/Cargo.toml");

    // Stdlib content version — independently tracked from the compiler.
    // Updated when std/*.mvl files have a meaningful release.
    println!("cargo:rustc-env=MVL_STDLIB_VERSION=0.54.0");

    // Build metadata for BuildInfo in std.runtime.
    let rustc_ver = rustc_version();
    let llvm_ver = llvm_version();
    let target = std::env::var("TARGET").unwrap_or_default();
    let profile = std::env::var("PROFILE").unwrap_or_default();
    let date = build_date();
    println!("cargo:rustc-env=MVL_RUSTC_VERSION={rustc_ver}");
    println!("cargo:rustc-env=MVL_LLVM_VERSION={llvm_ver}");
    println!("cargo:rustc-env=MVL_TARGET={target}");
    println!("cargo:rustc-env=MVL_PROFILE={profile}");
    println!("cargo:rustc-env=MVL_BUILD_DATE={date}");
}
