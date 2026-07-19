// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use std::collections::BTreeMap;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process;

// ── Data model ────────────────────────────────────────────────────────────────

#[derive(Default, Clone)]
struct FileCounts {
    files: usize,
    lines: usize,
    code: usize,
    comments: usize,
    blanks: usize,
    test_fns: usize,
}

impl FileCounts {
    fn add(&mut self, other: &FileCounts) {
        self.files += other.files;
        self.lines += other.lines;
        self.code += other.code;
        self.comments += other.comments;
        self.blanks += other.blanks;
        self.test_fns += other.test_fns;
    }
}

#[derive(Default)]
struct GroupCounts {
    src: FileCounts,
    tests: FileCounts,
}

impl GroupCounts {
    fn total(&self) -> FileCounts {
        let mut t = self.src.clone();
        t.add(&self.tests);
        t
    }

    fn add(&mut self, other: &GroupCounts) {
        self.src.add(&other.src);
        self.tests.add(&other.tests);
    }
}

// ── File walking & counting ───────────────────────────────────────────────────

fn is_test_file(path: &Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.ends_with("_test"))
        .unwrap_or(false)
}

fn count_test_fns(source: &str) -> usize {
    source
        .lines()
        .filter(|l| {
            let t = l.trim();
            t.starts_with("test fn ")
        })
        .count()
}

fn count_file(path: &Path) -> GroupCounts {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return GroupCounts::default(),
    };
    let mut fc = FileCounts {
        files: 1,
        test_fns: count_test_fns(&source),
        ..Default::default()
    };
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

// ── Formatting helpers ────────────────────────────────────────────────────────

fn fmt_num(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

struct Color {
    bold: &'static str,
    dim_green: &'static str,
    dim_yellow: &'static str,
    cyan_bold: &'static str,
    reset: &'static str,
}

const COLORS: Color = Color {
    bold: "\x1b[1m",
    dim_green: "\x1b[2;32m",
    dim_yellow: "\x1b[2;33m",
    cyan_bold: "\x1b[1;36m",
    reset: "\x1b[0m",
};

const NO_COLORS: Color = Color {
    bold: "",
    dim_green: "",
    dim_yellow: "",
    cyan_bold: "",
    reset: "",
};

fn colors() -> &'static Color {
    if std::io::stdout().is_terminal() && std::env::var("NO_COLOR").is_err() {
        &COLORS
    } else {
        &NO_COLORS
    }
}

// ── Table output ──────────────────────────────────────────────────────────────

// Width: 1 + 30 + 1 + 7 + 1 + 10 + 1 + 10 + 1 + 11 + 1 + 9 + 1 + 8 = 92
const WIDTH: usize = 92;

fn table_row(color: &'static str, reset: &'static str, label: &str, fc: &FileCounts) {
    println!(
        "{} {:<30} {:>7} {:>10} {:>10} {:>11} {:>9} {:>8}{}",
        color,
        label,
        fmt_num(fc.files),
        fmt_num(fc.lines),
        fmt_num(fc.code),
        fmt_num(fc.comments),
        fmt_num(fc.blanks),
        fmt_num(fc.test_fns),
        reset,
    );
}

fn print_table(root_label: &str, groups: &BTreeMap<String, GroupCounts>) {
    let c = colors();
    let sep = "━".repeat(WIDTH);
    let thin = "─".repeat(WIDTH);

    println!("{sep}");
    println!(
        "{} {:<30} {:>7} {:>10} {:>10} {:>11} {:>9} {:>8}{}",
        c.bold, "Directory", "Files", "Lines", "Code", "Comments", "Blanks", "TestFns", c.reset,
    );
    println!("{sep}");

    let mut grand_total = GroupCounts::default();

    for (label, g) in groups {
        let total = g.total();

        // Directory total row
        table_row(c.cyan_bold, c.reset, label, &total);

        // Sub-rows: only when both buckets are non-empty
        if g.src.files > 0 && g.tests.files > 0 {
            table_row(c.dim_green, c.reset, "  source", &g.src);
            table_row(c.dim_yellow, c.reset, "  tests", &g.tests);
        }

        grand_total.add(g);
    }

    let all = grand_total.total();
    println!("{thin}");
    table_row(c.bold, c.reset, &format!("Total  ({})", root_label), &all);
    println!("{sep}");
}

// ── CSV output ────────────────────────────────────────────────────────────────

fn print_csv(groups: &BTreeMap<String, GroupCounts>) {
    println!("directory,type,files,lines,code,comments,blanks,test_fns");

    let mut grand_total = GroupCounts::default();

    for (label, g) in groups {
        let total = g.total();
        let dir = if label.contains(',') { format!("\"{label}\"") } else { label.clone() };

        println!(
            "{},total,{},{},{},{},{},{}",
            dir, total.files, total.lines, total.code, total.comments, total.blanks, total.test_fns
        );
        if g.src.files > 0 {
            println!(
                "{},source,{},{},{},{},{},{}",
                dir,
                g.src.files,
                g.src.lines,
                g.src.code,
                g.src.comments,
                g.src.blanks,
                g.src.test_fns,
            );
        }
        if g.tests.files > 0 {
            println!(
                "{},tests,{},{},{},{},{},{}",
                dir,
                g.tests.files,
                g.tests.lines,
                g.tests.code,
                g.tests.comments,
                g.tests.blanks,
                g.tests.test_fns,
            );
        }

        grand_total.add(g);
    }

    let all = grand_total.total();
    println!(
        "Total,all,{},{},{},{},{},{}",
        all.files, all.lines, all.code, all.comments, all.blanks, all.test_fns
    );
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run(root: &str, csv: bool) {
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

    if csv {
        print_csv(&groups);
    } else {
        print_table(&root_label, &groups);
    }
}
