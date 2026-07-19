// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

#[derive(Default)]
struct Counts {
    files: usize,
    lines: usize,
    code: usize,
    comments: usize,
    blanks: usize,
}

impl Counts {
    fn add(&mut self, other: &Counts) {
        self.files += other.files;
        self.lines += other.lines;
        self.code += other.code;
        self.comments += other.comments;
        self.blanks += other.blanks;
    }
}

fn count_file(path: &Path) -> Counts {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return Counts::default(),
    };
    let mut counts = Counts { files: 1, ..Default::default() };
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
            result.extend(collect_mvl_files(&path));
        } else if path.extension().and_then(|e| e.to_str()) == Some("mvl") {
            result.push(path);
        }
    }
    result
}

pub fn run(root: &str) {
    let root_path = Path::new(root);
    if !root_path.is_dir() {
        eprintln!("error: '{}' is not a directory", root);
        process::exit(1);
    }

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
            let label = format!("{}/", name);
            let files = collect_mvl_files(&path);
            let group = groups.entry(label).or_default();
            for f in &files {
                group.add(&count_file(f));
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("mvl") {
            let group = groups.entry(".".to_string()).or_default();
            group.add(&count_file(&path));
        }
    }

    // Remove empty groups
    groups.retain(|_, v| v.files > 0);

    if groups.is_empty() {
        eprintln!("no .mvl files found in '{}'", root);
        process::exit(0);
    }

    let sep = "━".repeat(81);
    let thin = "─".repeat(81);
    println!("{sep}");
    println!(
        " {:<30} {:>7} {:>11} {:>11} {:>11} {:>11}",
        "Directory", "Files", "Lines", "Code", "Comments", "Blanks"
    );
    println!("{sep}");

    let mut total = Counts::default();
    for (label, c) in &groups {
        println!(
            " {:<30} {:>7} {:>11} {:>11} {:>11} {:>11}",
            label, c.files, c.lines, c.code, c.comments, c.blanks
        );
        total.add(c);
    }

    println!("{thin}");
    println!(
        " {:<30} {:>7} {:>11} {:>11} {:>11} {:>11}",
        "Total", total.files, total.lines, total.code, total.comments, total.blanks
    );
    println!("{sep}");
}
