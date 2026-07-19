// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

#[derive(Default, Clone)]
struct FileCounts {
    files: usize,
    lines: usize,
    code: usize,
    comments: usize,
    blanks: usize,
}

impl FileCounts {
    fn add(&mut self, other: &FileCounts) {
        self.files += other.files;
        self.lines += other.lines;
        self.code += other.code;
        self.comments += other.comments;
        self.blanks += other.blanks;
    }
}

#[derive(Default)]
struct GroupCounts {
    src: FileCounts,
    tests: FileCounts,
}

impl GroupCounts {
    fn add(&mut self, other: &GroupCounts) {
        self.src.add(&other.src);
        self.tests.add(&other.tests);
    }
}

fn is_test_file(path: &Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.ends_with("_test"))
        .unwrap_or(false)
}

fn count_file(path: &Path) -> GroupCounts {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return GroupCounts::default(),
    };
    let mut fc = FileCounts { files: 1, ..Default::default() };
    for line in source.lines() {
        fc.lines += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            fc.blanks += 1;
        } else if trimmed.starts_with("///")
            || trimmed.starts_with("//!")
            || trimmed.starts_with("//")
        {
            fc.comments += 1;
        } else {
            fc.code += 1;
        }
    }
    if is_test_file(path) {
        GroupCounts { src: FileCounts::default(), tests: fc }
    } else {
        GroupCounts { src: fc, tests: FileCounts::default() }
    }
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

// Width: 1 + 32 + 1 + 7 + 1 + 10 + 1 + 10 + 1 + 10 + 1 + 10 = 85
const WIDTH: usize = 85;

fn row(dir: &str, c: &FileCounts) {
    println!(
        " {:<32} {:>7} {:>10} {:>10} {:>10} {:>10}",
        dir, c.files, c.lines, c.code, c.comments, c.blanks
    );
}

fn section(title: &str, groups: &BTreeMap<String, GroupCounts>, get: impl Fn(&GroupCounts) -> &FileCounts) {
    let sep = "━".repeat(WIDTH);
    let thin = "─".repeat(WIDTH);

    let relevant: Vec<_> = groups.iter().filter(|(_, g)| get(g).files > 0).collect();
    if relevant.is_empty() {
        return;
    }

    println!("{sep}");
    println!(" {:<32} {:>7} {:>10} {:>10} {:>10} {:>10}", title, "Files", "Lines", "Code", "Comments", "Blanks");
    println!("{sep}");

    let mut subtotal = FileCounts::default();
    for (label, g) in &relevant {
        let c = get(g);
        row(label, c);
        subtotal.add(c);
    }

    println!("{thin}");
    row("Subtotal", &subtotal);
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

    let mut groups: BTreeMap<String, GroupCounts> = BTreeMap::new();

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

    groups.retain(|_, g| g.src.files + g.tests.files > 0);

    if groups.is_empty() {
        eprintln!("no .mvl files found in '{}'", root);
        process::exit(0);
    }

    section("Source", &groups, |g| &g.src);
    section("Tests", &groups, |g| &g.tests);

    // Grand total
    let mut total = GroupCounts::default();
    for g in groups.values() {
        total.add(g);
    }
    let sep = "━".repeat(WIDTH);
    println!("{sep}");
    let mut all = total.src.clone();
    all.add(&total.tests);
    row("Total", &all);
    println!("{sep}");
}
