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
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus_old")
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

// ── Unit tests for the harness itself ────────────────────────────────────────
//
// These guard against silent breakage in `MVL_UPDATE_PARITY_BASELINE=1` mode,
// where a broken `check_file` (e.g. always returning ParseError) would happily
// write a bogus baseline that then passes the outer test forever.

#[test]
fn format_line_ok() {
    let v = FileVerdict {
        rel_path: "tests/corpus/a.mvl".into(),
        kind: VerdictKind::Ok,
    };
    assert_eq!(v.format_line(), "tests/corpus/a.mvl\tok");
}

#[test]
fn format_line_parse_error() {
    let v = FileVerdict {
        rel_path: "tests/corpus/bad.mvl".into(),
        kind: VerdictKind::ParseError,
    };
    assert_eq!(v.format_line(), "tests/corpus/bad.mvl\tparse-error");
}

#[test]
fn format_line_fail_sorts_and_joins_requirements() {
    let v = FileVerdict {
        rel_path: "tests/corpus/x.mvl".into(),
        kind: VerdictKind::Fail {
            error_count: 3,
            requirements: vec![1, 7, 11],
        },
    };
    assert_eq!(v.format_line(), "tests/corpus/x.mvl\tfail\t3\t1,7,11");
}

#[test]
fn serialize_sorts_lines_and_includes_header() {
    let verdicts = vec![
        FileVerdict {
            rel_path: "z.mvl".into(),
            kind: VerdictKind::Ok,
        },
        FileVerdict {
            rel_path: "a.mvl".into(),
            kind: VerdictKind::ParseError,
        },
        FileVerdict {
            rel_path: "m.mvl".into(),
            kind: VerdictKind::Fail {
                error_count: 1,
                requirements: vec![7],
            },
        },
    ];
    let s = serialize(&verdicts);
    assert!(s.starts_with("# Checker parity baseline"), "missing header");
    assert!(s.ends_with('\n'), "must end with newline");
    let body: Vec<&str> = s.lines().skip(1).collect();
    assert_eq!(
        body,
        vec!["a.mvl\tparse-error", "m.mvl\tfail\t1\t7", "z.mvl\tok"],
    );
}

#[test]
fn walk_finds_mvl_files_recursively_and_skips_others() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("top.mvl"), "").unwrap();
    std::fs::write(root.join("sub").join("nested.mvl"), "").unwrap();
    std::fs::write(root.join("ignored.rs"), "").unwrap();
    std::fs::write(root.join("no_ext"), "").unwrap();

    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    let names: Vec<String> = out
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, vec!["nested.mvl", "top.mvl"]);
}

#[test]
fn check_file_returns_ok_for_valid_program() {
    let tmp = tempfile::NamedTempFile::with_suffix(".mvl").expect("tempfile");
    std::fs::write(tmp.path(), "fn main() -> Unit { }\n").unwrap();
    assert_eq!(check_file(tmp.path()), VerdictKind::Ok);
}

#[test]
fn check_file_returns_parse_error_for_garbage() {
    let tmp = tempfile::NamedTempFile::with_suffix(".mvl").expect("tempfile");
    // Missing braces / clearly not MVL — the parser should reject.
    std::fs::write(tmp.path(), "this is not mvl syntax @@@\n").unwrap();
    assert_eq!(check_file(tmp.path()), VerdictKind::ParseError);
}

#[test]
fn check_file_returns_fail_for_type_error() {
    let tmp = tempfile::NamedTempFile::with_suffix(".mvl").expect("tempfile");
    // Type mismatch — Int assigned to a String binding.  Requirement 1
    // (types) should be the one reported.
    std::fs::write(tmp.path(), "fn main() -> Unit { let x: String = 42; }\n").unwrap();
    match check_file(tmp.path()) {
        VerdictKind::Fail {
            error_count,
            requirements,
        } => {
            assert!(error_count >= 1, "expected >=1 error, got {error_count}");
            assert!(
                requirements.contains(&1),
                "expected requirement 1 among {requirements:?}",
            );
        }
        other => panic!("expected Fail, got {other:?}"),
    }
}
