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
        "init" => {
            cmd_self_init();
        }
        other => {
            if other.is_empty() {
                eprintln!("Usage: mvl self <init|install|use|list|uninstall>");
            } else {
                eprintln!("Unknown self subcommand: {other}");
                eprintln!("Usage: mvl self <init|install|use|list|uninstall>");
            }
            process::exit(1);
        }
    }
}

/// `mvl self init` — extract the bundled stdlib to the toolchain directory.
fn cmd_self_init() {
    let path = stdlib::ensure_stdlib();
    println!(
        "mvl stdlib v{} ready at {}",
        stdlib::STDLIB_VERSION,
        path.display()
    );
}

pub(super) fn cmd_pkg_add(args: &[String]) {
    let pkg_id = args.get(2).unwrap_or_else(|| {
        eprintln!(
            "Usage: mvl add <pkg-id> [<tag>] [--rationale \"...\"] [--allow-license \"...\"]"
        );
        eprintln!("  pkg-id:          git URL or github.com/user/repo style identifier");
        eprintln!("  tag:             optional version tag (e.g. v1.2.0); omit to use latest");
        eprintln!("  --rationale:     justification for adding this dependency");
        eprintln!(
            "  --allow-license: override a license policy rejection (reason logged in mvl.lock)"
        );
        process::exit(1);
    });
    // Parse positional tag (first arg after pkg-id that doesn't start with --)
    let tag = args
        .get(3)
        .filter(|a| !a.starts_with("--"))
        .map(|s| s.as_str());
    // Parse --rationale flag
    let rationale = args
        .windows(2)
        .find(|w| w[0] == "--rationale")
        .map(|w| w[1].as_str());
    // Parse --allow-license flag (#635)
    let allow_license = args
        .windows(2)
        .find(|w| w[0] == "--allow-license")
        .map(|w| w[1].as_str());
    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Err(e) = packages::cmd_add(pkg_id, tag, rationale, allow_license, &project_root) {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

pub(super) fn cmd_sbom(args: &[String]) {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!("Usage: mvl sbom [--format=cyclonedx|spdx] [--output=<file>]");
        eprintln!("  Generate a software bill of materials from mvl.toml + mvl.lock.");
        eprintln!("  --format=cyclonedx   CycloneDX 1.5 JSON (default)");
        eprintln!("  --format=spdx        SPDX 2.3 tag-value");
        eprintln!("  --output=<file>      write to file instead of stdout");
        eprintln!("  Run from a project directory containing mvl.toml.");
        return;
    }

    let format = args.iter().find_map(|a| a.strip_prefix("--format="));
    let output = args.iter().find_map(|a| a.strip_prefix("--output="));

    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match packages::cmd_sbom(format, &project_root) {
        Err(e) => {
            eprintln!("error: {e}");
            if matches!(e, packages::PackageError::Manifest(_)) {
                eprintln!("hint: run 'mvl sbom' from a project directory containing mvl.toml");
            }
            process::exit(1);
        }
        Ok(doc) => match output {
            None => print!("{doc}"),
            Some(path) => {
                if let Err(e) = std::fs::write(path, &doc) {
                    eprintln!("error: cannot write {path}: {e}");
                    process::exit(1);
                }
                println!("SBOM written to {path}");
            }
        },
    }
}

pub(super) fn cmd_audit(args: &[String]) {
    let paradox = args.iter().any(|a| a == "--paradox");
    let supply_chain = args.iter().any(|a| a == "--supply-chain");
    let license = args.iter().any(|a| a == "--license");

    let flag_count = [paradox, supply_chain, license]
        .iter()
        .filter(|&&f| f)
        .count();

    if flag_count > 1 {
        eprintln!("error: --paradox, --supply-chain, and --license are mutually exclusive");
        process::exit(1);
    }

    if flag_count == 0 {
        eprintln!("Usage: mvl audit <--paradox | --supply-chain | --license>");
        eprintln!("  --paradox:       audit dependencies for the Dependency Paradox policy");
        eprintln!("                   exits with code 1 if any dep below complexity threshold lacks rationale");
        eprintln!(
            "  --supply-chain:  scan [native] and [c-native] deps against NVD/OSV for CVEs (#633)"
        );
        eprintln!("                   exits with code 1 if any vulnerability is found");
        eprintln!("  --license:       check dependency licenses against project policy (#635)");
        eprintln!(
            "                   exits with code 1 if any license is rejected; warns on unknown"
        );
        eprintln!();
        eprintln!("Environment variables:");
        eprintln!("  NVD_API_KEY      NVD API key for higher rate limits (60 req/min vs 5/min)");
        process::exit(1);
    }

    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    if paradox {
        match packages::cmd_audit_paradox(&project_root) {
            Err(e) => {
                eprintln!("error: {e}");
                process::exit(1);
            }
            Ok(audit) => {
                print!("{}", audit.render());
                if audit.has_violations() {
                    process::exit(1);
                }
            }
        }
    }

    if supply_chain {
        match packages::cmd_audit_supply_chain(&project_root) {
            Err(e) => {
                eprintln!("error: {e}");
                process::exit(1);
            }
            Ok(audit) => {
                print!("{}", audit.render());
                if audit.has_vulnerabilities() {
                    process::exit(1);
                }
            }
        }
    }

    if license {
        match packages::cmd_audit_license(&project_root) {
            Err(e) => {
                eprintln!("error: {e}");
                process::exit(1);
            }
            Ok(audit) => {
                print!("{}", audit.render());
                if audit.has_violations() {
                    process::exit(1);
                }
            }
        }
    }
}

/// `mvl init [<name>]` — scaffold a new MVL project in the current directory.
///
/// Creates `mvl.toml` and `src/main.mvl`. The package name defaults to the
/// current directory name if not provided.
pub(super) fn cmd_init(args: &[String]) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // Refuse if mvl.toml already exists
    if cwd.join("mvl.toml").exists() {
        eprintln!("error: mvl.toml already exists in this directory");
        eprintln!("hint: use 'mvl add' to add dependencies or edit mvl.toml directly");
        process::exit(1);
    }

    // Derive the package name: explicit arg > current directory name
    let name = if let Some(n) = args.get(2).filter(|a| !a.starts_with('-')) {
        n.clone()
    } else {
        cwd.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("my-project")
            .to_string()
    };

    let mvl_version = env!("CARGO_PKG_VERSION");
    let toml = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nlicense = \"MIT\"\nrequires-mvl = \">={mvl_version}\"\n"
    );

    let main_mvl =
        "//! Entry point.\n\nfn main() -> Unit ! Console {\n    println(\"Hello from {name}!\")\n}\n"
            .replace("{name}", &name);

    // Write mvl.toml
    if let Err(e) = std::fs::write(cwd.join("mvl.toml"), &toml) {
        eprintln!("error: cannot write mvl.toml: {e}");
        process::exit(1);
    }

    // Write src/main.mvl
    let src_dir = cwd.join("src");
    if let Err(e) = std::fs::create_dir_all(&src_dir) {
        eprintln!("error: cannot create src/: {e}");
        process::exit(1);
    }
    if let Err(e) = std::fs::write(src_dir.join("main.mvl"), &main_mvl) {
        eprintln!("error: cannot write src/main.mvl: {e}");
        process::exit(1);
    }

    println!("Created MVL project '{name}'");
    println!("  mvl.toml");
    println!("  src/main.mvl");
    println!();
    println!("Next steps:");
    println!("  mvl check src/main.mvl   — type-check");
    println!("  mvl sbom                  — generate SBOM");
    println!("  mvl add <pkg>             — add a dependency");
}
