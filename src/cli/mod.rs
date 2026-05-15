// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

pub mod assurance;
pub mod build;
pub mod check;
pub mod complexity;
pub mod lint;
pub mod mcdc;
pub mod mutate;
pub mod test;
pub mod transpile;

use std::fs;
use std::path::Path;

// ── Pure-MVL stdlib files verified in proven mode (ADR-0023, #538) ───────────
//
// These files contain pure MVL function bodies that can be verified against
// all 11 requirements.  OS/hardware-backed modules (io, env, process, crypto,
// random, time, regex, args, log) are excluded — they are only `pub builtin fn`
// declarations with no body to check.
pub(super) const PROVEN_STDLIB_FILES: &[&str] = &[
    "core.mvl",
    "strings.mvl",
    "lists.mvl",
    "math.mvl",
    "collections.mvl",
    "json.mvl",
    // pbt.mvl: excluded pending checker fix for while-loop return type in
    // generic match arms (#538 follow-up, tracked separately)
];

pub(super) fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path)?;
        } else {
            fs::copy(entry.path(), dest_path)?;
        }
    }
    Ok(())
}

pub(super) fn json_escape(s: &str) -> String {
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
