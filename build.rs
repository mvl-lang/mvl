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

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let std_dir = Path::new(&manifest_dir).join("std");
    let out_dir = std::env::var("OUT_DIR").unwrap();

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
