// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::packages;
use mvl::mvl::stdlib;
use mvl::mvl::toolchain;
use std::path::PathBuf;
use std::process;

pub(super) fn cmd_self(args: &[String]) {
    let subcmd = args.get(2).map(|s| s.as_str()).unwrap_or("");
    match subcmd {
        "install" => {
            let version = args.get(3).unwrap_or_else(|| {
                eprintln!("Usage: mvl self install <version>");
                process::exit(1);
            });
            toolchain::cmd_self_install(version);
        }
        "use" => {
            let version = args.get(3).unwrap_or_else(|| {
                eprintln!("Usage: mvl self use <version>");
                process::exit(1);
            });
            toolchain::cmd_self_use(version);
        }
        "list" => {
            toolchain::cmd_self_list();
        }
        "uninstall" => {
            let version = args.get(3).unwrap_or_else(|| {
                eprintln!("Usage: mvl self uninstall <version>");
                process::exit(1);
            });
            toolchain::cmd_self_uninstall(version);
        }
        other => {
            if other.is_empty() {
                eprintln!("Usage: mvl self <install|use|list|uninstall>");
            } else {
                eprintln!("Unknown self subcommand: {other}");
                eprintln!("Usage: mvl self <install|use|list|uninstall>");
            }
            process::exit(1);
        }
    }
}

pub(super) fn cmd_pkg_add(args: &[String]) {
    let pkg_id = args.get(2).unwrap_or_else(|| {
        eprintln!("Usage: mvl add <pkg-id> [<tag>]");
        eprintln!("  pkg-id: git URL or github.com/user/repo style identifier");
        eprintln!("  tag:    optional version tag (e.g. v1.2.0); omit to use latest");
        process::exit(1);
    });
    let tag = args.get(3).map(|s| s.as_str());
    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    packages::cmd_add(pkg_id, tag, &project_root);
}

pub(super) fn cmd_init() {
    let path = stdlib::ensure_stdlib();
    println!(
        "mvl stdlib v{} ready at {}",
        stdlib::STDLIB_VERSION,
        path.display()
    );
}
