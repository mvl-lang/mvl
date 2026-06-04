use std::fs;
use std::path::Path;

/// Recursively collect all `.mvl` files under `dir`.
/// `rel_prefix` is the path prefix relative to `std/` (empty for the top level).
/// Returns `(relative_name, absolute_path)` pairs, e.g. `("kv/file.mvl", "/abs/path")`.
fn collect_mvl_files(dir: &Path, rel_prefix: &str, entries: &mut Vec<(String, String)>) {
    let read = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for entry in read.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            let dir_name = path.file_name().unwrap().to_str().unwrap();
            let new_prefix = if rel_prefix.is_empty() {
                dir_name.to_string()
            } else {
                format!("{rel_prefix}/{dir_name}")
            };
            collect_mvl_files(&path, &new_prefix, entries);
        } else if path.extension().is_some_and(|x| x == "mvl") {
            let file_name = path.file_name().unwrap().to_str().unwrap();
            let rel_name = if rel_prefix.is_empty() {
                file_name.to_string()
            } else {
                format!("{rel_prefix}/{file_name}")
            };
            let abs = path.canonicalize().unwrap();
            println!("cargo:rerun-if-changed={}", abs.display());
            entries.push((rel_name, abs.to_str().unwrap().to_string()));
        }
    }
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
    let std_dir = Path::new(&manifest_dir).join("std");
    let out_dir = std::env::var("OUT_DIR").unwrap();

    // Expose the mvl_runtime crate version so manifest_embed.rs can embed it.
    let runtime_toml = Path::new(&manifest_dir).join("runtime/rust/Cargo.toml");
    println!(
        "cargo:rustc-env=MVL_RUNTIME_VERSION={}",
        read_toml_version(&runtime_toml)
    );
    println!("cargo:rerun-if-changed=runtime/rust/Cargo.toml");

    // Stdlib content version — independently tracked from the compiler.
    // Updated when std/*.mvl files have a meaningful release.
    println!("cargo:rustc-env=MVL_STDLIB_VERSION=0.42.0");

    println!("cargo:rerun-if-changed=std/");

    let mut entries: Vec<(String, String)> = Vec::new();
    collect_mvl_files(&std_dir, "", &mut entries);

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let lines: Vec<String> = entries
        .iter()
        .map(|(name, path)| format!("    ({name:?}, include_str!({path:?}))"))
        .collect();

    let code = format!("&[\n{}\n]", lines.join(",\n"));
    fs::write(Path::new(&out_dir).join("stdlib_files.rs"), code).unwrap();
}
