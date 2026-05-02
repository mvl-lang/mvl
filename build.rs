use std::fs;
use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let std_dir = Path::new(&manifest_dir).join("std");
    let out_dir = std::env::var("OUT_DIR").unwrap();

    println!("cargo:rerun-if-changed=std/");

    let mut entries: Vec<(String, String)> = fs::read_dir(&std_dir)
        .expect("std/ directory not found")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "mvl"))
        .map(|e| {
            let path = e.path();
            let name = path.file_name().unwrap().to_str().unwrap().to_string();
            let abs = path.canonicalize().unwrap();
            println!("cargo:rerun-if-changed={}", abs.display());
            (name, abs.to_str().unwrap().to_string())
        })
        .collect();

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let lines: Vec<String> = entries
        .iter()
        .map(|(name, path)| format!("    ({name:?}, include_str!({path:?}))"))
        .collect();

    let code = format!("&[\n{}\n]", lines.join(",\n"));
    fs::write(Path::new(&out_dir).join("stdlib_files.rs"), code).unwrap();
}
