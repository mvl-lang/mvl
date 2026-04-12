// bridge.rs — Rust implementations of the extern "rust" trust boundary.
//
// These functions are declared in main.mvl's `extern "rust"` block.
// MVL trusts their signatures but does not verify their bodies.
// Keep this file minimal — every line here is unverified Rust.

use mvl_runtime::prelude::*;

/// Read a log file from disk; return its raw contents as tainted (unvalidated) data.
#[no_mangle]
pub extern "Rust" fn read_log_file(path: String) -> Tainted<String> {
    let contents = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("error: cannot read {path}: {e}");
        std::process::exit(1);
    });
    Tainted(contents)
}

/// Count ERROR/WARN lines in the (already sanitized) log content and return a report.
#[no_mangle]
pub extern "Rust" fn count_and_format(content: Clean<String>) -> String {
    let text = &**content;
    let lines: Vec<&str> = text.lines().collect();
    let errors = lines.iter().filter(|l| l.contains("ERROR")).count();
    let warnings = lines.iter().filter(|l| l.contains("WARN")).count();
    format!(
        "Log analysis: {} lines, {} errors, {} warnings",
        lines.len(),
        errors,
        warnings
    )
}
