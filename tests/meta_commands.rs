//! Integration tests for `mvl init`, `mvl sbom --output`, and `mvl sbom --help`.
//!
//! Covers spec 024 R6 (SBOM File Output), R7 (Project Scaffolding), and R9 (SBOM Help Flag).
//! `tests/stdlib.rs` covers the disk-based stdlib loader (#1765 replaces the
//! extraction flow — R8 no longer applies).

use std::process::Command;

fn mvl_bin() -> std::path::PathBuf {
    let mut p = std::env::current_exe().expect("current_exe");
    p.pop(); // test binary
    p.pop(); // deps/
    p.push("mvl");
    p
}

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let mut p = std::env::temp_dir();
        p.push(format!("mvl-meta-{name}-{}", std::process::id()));
        // Wipe any stale dir from a previous run.
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("create tempdir");
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn mvl_init_creates_skeleton() {
    // Spec 024 R7: `mvl init` creates mvl.toml + src/main.mvl in cwd.
    let tmp = TempDir::new("init");
    let out = Command::new(mvl_bin())
        .arg("init")
        .current_dir(&tmp.0)
        .output()
        .expect("run mvl init");
    assert!(out.status.success(), "mvl init failed: {:?}", out);
    assert!(tmp.0.join("mvl.toml").is_file(), "mvl.toml not created");
    assert!(
        tmp.0.join("src").join("main.mvl").is_file(),
        "src/main.mvl not created"
    );
    let toml = std::fs::read_to_string(tmp.0.join("mvl.toml")).expect("read mvl.toml");
    assert!(
        toml.contains("name ="),
        "mvl.toml missing name field:\n{toml}"
    );
    assert!(
        toml.contains("version ="),
        "mvl.toml missing version field:\n{toml}"
    );
}

#[test]
fn mvl_init_rejects_existing_mvl_toml() {
    // Spec 024 R7: if mvl.toml exists, exit non-zero with a hint.
    let tmp = TempDir::new("init-existing");
    std::fs::write(tmp.0.join("mvl.toml"), "[package]\nname = \"existing\"\n").unwrap();
    let out = Command::new(mvl_bin())
        .arg("init")
        .current_dir(&tmp.0)
        .output()
        .expect("run mvl init");
    assert!(
        !out.status.success(),
        "expected exit non-zero, got: {:?}",
        out
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("already exists"),
        "expected hint in stderr:\n{stderr}"
    );
}

#[test]
fn mvl_sbom_help_exits_zero() {
    // Spec 024 R9: `mvl sbom --help` prints usage and exits 0.
    let out = Command::new(mvl_bin())
        .args(["sbom", "--help"])
        .output()
        .expect("run mvl sbom --help");
    assert!(out.status.success(), "sbom --help failed: {:?}", out);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("sbom"),
        "expected 'sbom' in help output:\n{combined}"
    );
    assert!(
        combined.contains("--format") || combined.contains("--output"),
        "expected flag documentation in help output:\n{combined}"
    );
}

#[test]
fn mvl_sbom_writes_to_output_file() {
    // Spec 024 R6: `mvl sbom --output=<file>` writes to file and confirms on stdout.
    let tmp = TempDir::new("sbom-output");
    // Need a minimal project: run `mvl init` first to create mvl.toml.
    let init = Command::new(mvl_bin())
        .arg("init")
        .current_dir(&tmp.0)
        .output()
        .expect("run mvl init");
    assert!(init.status.success(), "mvl init failed: {:?}", init);

    let sbom_path = tmp.0.join("sbom.json");
    let out = Command::new(mvl_bin())
        .args(["sbom", &format!("--output={}", sbom_path.display())])
        .current_dir(&tmp.0)
        .output()
        .expect("run mvl sbom --output=…");
    assert!(out.status.success(), "mvl sbom --output failed: {:?}", out);
    assert!(
        sbom_path.is_file(),
        "SBOM file not written at {}",
        sbom_path.display()
    );
    let contents = std::fs::read_to_string(&sbom_path).expect("read SBOM file");
    assert!(
        contents.contains("bomFormat") || contents.contains("CycloneDX"),
        "expected CycloneDX content in SBOM file:\n{contents}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("written to") || stdout.contains(sbom_path.to_string_lossy().as_ref()),
        "expected confirmation message on stdout:\n{stdout}"
    );
}
