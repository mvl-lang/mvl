// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Self-hosting checker parity harness (#1117 slice 1).
//!
//! Records the Rust checker's per-file verdict across `tests/corpus/` in a
//! versioned baseline.  Purpose: as the MVL self-hosted checker in
//! `compiler/` is ported module-by-module, a follow-up test will diff the
//! MVL-checker output against this baseline to ratchet down divergence.
//!
//! For now only the Rust side runs — this file locks in the *stable*
//! behavior we want the MVL checker to eventually match.  Any change to
//! corpus verdicts must be an intentional baseline update.
//!
//! Regenerate baseline: `MVL_UPDATE_PARITY_BASELINE=1 cargo test --test checker_parity`

use std::path::{Path, PathBuf};

use mvl::mvl::checker;
use mvl::mvl::loader;
use mvl::mvl::parser::Parser;

/// Line format in the baseline file:
///
/// ```text
/// <relative-path>\tparse-error
/// <relative-path>\tok
/// <relative-path>\tfail\t<error_count>\t<comma-separated sorted requirement numbers>
/// ```
///
/// Sorted alphabetically by path.  One line per corpus `.mvl` file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileVerdict {
    rel_path: String,
    kind: VerdictKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum VerdictKind {
    ParseError,
    Ok,
    Fail {
        error_count: usize,
        requirements: Vec<u8>,
    },
}

impl FileVerdict {
    fn format_line(&self) -> String {
        match &self.kind {
            VerdictKind::ParseError => format!("{}\tparse-error", self.rel_path),
            VerdictKind::Ok => format!("{}\tok", self.rel_path),
            VerdictKind::Fail {
                error_count,
                requirements,
            } => {
                let reqs = requirements
                    .iter()
                    .map(|r| r.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{}\tfail\t{}\t{}", self.rel_path, error_count, reqs)
            }
        }
    }
}

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/checker_parity/baseline.tsv")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|e| e == "mvl") {
            out.push(p);
        }
    }
}

fn all_corpus_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(&corpus_root(), &mut out);
    out.sort();
    out
}

fn check_file(file: &Path) -> VerdictKind {
    let Ok(src) = std::fs::read_to_string(file) else {
        return VerdictKind::ParseError;
    };
    let (mut p, lex_errs) = Parser::new(&src);
    if !lex_errs.is_empty() {
        return VerdictKind::ParseError;
    }
    let prog = p.parse_program();
    if !p.errors().is_empty() {
        return VerdictKind::ParseError;
    }

    let mut prelude = loader::load_implicit_prelude();
    prelude.extend(loader::load_mvl_native_stdlib_extras(std::slice::from_ref(
        &prog,
    )));
    prelude.extend(loader::load_rust_backed_stdlib_fns(std::slice::from_ref(
        &prog,
    )));
    let result = checker::check_with_prelude(&prelude, &prog);

    if result.is_ok() {
        VerdictKind::Ok
    } else {
        let mut reqs: Vec<u8> = result
            .errors
            .iter()
            .map(|e| e.requirement_number())
            .collect();
        reqs.sort_unstable();
        reqs.dedup();
        VerdictKind::Fail {
            error_count: result.errors.len(),
            requirements: reqs,
        }
    }
}

fn compute_current() -> Vec<FileVerdict> {
    let root = corpus_root();
    let root_str = root.to_string_lossy().to_string();
    let manifest = env!("CARGO_MANIFEST_DIR");

    all_corpus_files()
        .into_iter()
        .map(|f| {
            let rel = f
                .strip_prefix(manifest)
                .unwrap_or(&f)
                .to_string_lossy()
                .replace('\\', "/");
            let _ = &root_str;
            FileVerdict {
                rel_path: rel,
                kind: check_file(&f),
            }
        })
        .collect()
}

fn serialize(verdicts: &[FileVerdict]) -> String {
    let mut lines: Vec<String> = verdicts.iter().map(|v| v.format_line()).collect();
    lines.sort();
    let mut out = String::from(
        "# Checker parity baseline (#1117).  Regenerate: MVL_UPDATE_PARITY_BASELINE=1 cargo test --test checker_parity\n",
    );
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
    out
}

#[test]
fn checker_parity_baseline_stable() {
    let current = compute_current();
    assert!(
        !current.is_empty(),
        "no corpus files found — wrong working directory?"
    );
    let serialized = serialize(&current);

    let path = baseline_path();
    if std::env::var("MVL_UPDATE_PARITY_BASELINE").is_ok() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create baseline dir");
        }
        std::fs::write(&path, &serialized).expect("write baseline");
        eprintln!("regenerated {} ({} entries)", path.display(), current.len());
        return;
    }

    let existing = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "baseline not found at {} — run with MVL_UPDATE_PARITY_BASELINE=1 to create",
            path.display()
        )
    });

    if existing != serialized {
        // Show first 20 lines of unified diff-style output.
        let mut diffs = Vec::new();
        let expected: Vec<&str> = existing.lines().collect();
        let actual: Vec<&str> = serialized.lines().collect();
        let mut i = 0usize;
        let mut j = 0usize;
        while i < expected.len() && j < actual.len() && diffs.len() < 40 {
            if expected[i] == actual[j] {
                i += 1;
                j += 1;
            } else {
                diffs.push(format!("- {}", expected[i]));
                diffs.push(format!("+ {}", actual[j]));
                i += 1;
                j += 1;
            }
        }
        while i < expected.len() && diffs.len() < 40 {
            diffs.push(format!("- {}", expected[i]));
            i += 1;
        }
        while j < actual.len() && diffs.len() < 40 {
            diffs.push(format!("+ {}", actual[j]));
            j += 1;
        }
        panic!(
            "checker parity baseline drift.\n\nFirst divergences:\n{}\n\nIf intentional, regenerate: MVL_UPDATE_PARITY_BASELINE=1 cargo test --test checker_parity",
            diffs.join("\n")
        );
    }
}
