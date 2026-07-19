// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

#[derive(Default)]
struct Counts {
    src: usize,
    tests: usize,
    lines: usize,
    code: usize,
    comments: usize,
    blanks: usize,
}

impl Counts {
    fn files(&self) -> usize {
        self.src + self.tests
    }

    fn add(&mut self, other: &Counts) {
        self.src += other.src;
        self.tests += other.tests;
        self.lines += other.lines;
        self.code += other.code;
        self.comments += other.comments;
        self.blanks += other.blanks;
    }
}

fn is_test_file(path: &Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.ends_with("_test"))
        .unwrap_or(false)
}

fn count_file(path: &Path) -> Counts {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return Counts::default(),
    };
    let is_test = is_test_file(path);
    let mut counts = Counts {
        src: usize::from(!is_test),
        tests: usize::from(is_test),
        ..Default::default()
    };
    for line in source.lines() {
        counts.lines += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            counts.blanks += 1;
        } else if trimmed.starts_with("///")
            || trimmed.starts_with("//!")
            || trimmed.starts_with("//")
        {
            counts.comments += 1;
        } else {
            counts.code += 1;
        }
    }
    counts
}

fn collect_mvl_files(dir: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return result,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip hidden directories (e.g. .mvl/ package cache)
            let is_hidden = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with('.'))
                .unwrap_or(false);
            if !is_hidden {
                result.extend(collect_mvl_files(&path));
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("mvl") {
            result.push(path);
        }
    }
    result
}

// Width: 1 + 32 + 1 + 5 + 1 + 6 + 1 + 10 + 1 + 10 + 1 + 10 + 1 + 10 = 90
const WIDTH: usize = 90;

fn row(dir: &str, src: usize, tests: usize, lines: usize, code: usize, comments: usize, blanks: usize) {
    println!(" {:<32} {:>5} {:>6} {:>10} {:>10} {:>10} {:>10}", dir, src, tests, lines, code, comments, blanks);
}

pub fn run(root: &str) {
    let root_path = Path::new(root);
    if !root_path.is_dir() {
        eprintln!("error: '{}' is not a directory", root);
        process::exit(1);
    }

    let root_label = root_path
        .canonicalize()
        .ok()
        .as_deref()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or(root)
        .to_string();

    // Group files by immediate subdirectory (depth 1 from root)
    let mut groups: BTreeMap<String, Counts> = BTreeMap::new();

    let entries = match fs::read_dir(root_path) {
        Ok(e) => e,
        Err(err) => {
            eprintln!("error: cannot read directory '{}': {}", root, err);
            process::exit(1);
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        if path.is_dir() {
            // Skip hidden directories (e.g. .mvl/ package cache)
            if name.starts_with('.') {
                continue;
            }
            let label = format!("{}/", name);
            let files = collect_mvl_files(&path);
            let group = groups.entry(label).or_default();
            for f in &files {
                group.add(&count_file(f));
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("mvl") {
            let group = groups.entry(format!("({root_label})")).or_default();
            group.add(&count_file(&path));
        }
    }

    // Remove empty groups
    groups.retain(|_, v| v.files() > 0);

    if groups.is_empty() {
        eprintln!("no .mvl files found in '{}'", root);
        process::exit(0);
    }

    let sep = "━".repeat(WIDTH);
    let thin = "─".repeat(WIDTH);
    println!("{sep}");
    println!(" {:<32} {:>5} {:>6} {:>10} {:>10} {:>10} {:>10}", "Directory", "Src", "Tests", "Lines", "Code", "Comments", "Blanks");
    println!("{sep}");

    let mut total = Counts::default();
    for (label, c) in &groups {
        row(label, c.src, c.tests, c.lines, c.code, c.comments, c.blanks);
        total.add(c);
    }

    println!("{thin}");
    row("Total", total.src, total.tests, total.lines, total.code, total.comments, total.blanks);
    println!("{sep}");
}
