use mvl::mvl::stdlib::{ensure_stdlib, STDLIB_FILES, STDLIB_VERSION};
use std::path::PathBuf;

fn versioned_stdlib_path(home: &std::path::Path) -> PathBuf {
    home.join("toolchains").join(STDLIB_VERSION).join("std")
}
use std::fs;
use std::sync::{LazyLock, Mutex};

// Serialize all tests that mutate the MVL_HOME env var, since env vars are
// process-global and test threads run concurrently by default.
static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// RAII guard that removes MVL_HOME on drop, even if the test panics.
struct MvlHomeGuard;
impl Drop for MvlHomeGuard {
    fn drop(&mut self) {
        std::env::remove_var("MVL_HOME");
    }
}

/// Set MVL_HOME to a temp dir, run the closure, then clear the env var.
/// Acquires ENV_LOCK to prevent concurrent mutation of MVL_HOME.
/// MVL_HOME is removed via a Drop guard so it is cleaned up even on panic.
fn with_mvl_home<F: FnOnce(&std::path::Path)>(f: F) {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("tempdir");
    // MVL_HOME points at the data root; ensure_stdlib() appends "toolchains/{version}/std"
    std::env::set_var("MVL_HOME", tmp.path());
    let _cleanup = MvlHomeGuard;
    f(tmp.path());
}

#[test]
fn fresh_extraction_creates_files_and_stamp() {
    with_mvl_home(|home| {
        let stdlib_dir = ensure_stdlib();

        assert!(
            stdlib_dir.exists(),
            "stdlib dir must be created: {}",
            stdlib_dir.display()
        );
        assert_eq!(
            stdlib_dir,
            versioned_stdlib_path(home),
            "stdlib dir must be under MVL_HOME/toolchains/VERSION/std"
        );

        for (name, _) in STDLIB_FILES {
            assert!(
                stdlib_dir.join(name).exists(),
                "stdlib file {name} must be extracted"
            );
        }

        let stamp = fs::read_to_string(stdlib_dir.join(".version"))
            .expect(".version stamp must be written");
        assert_eq!(
            stamp.trim(),
            STDLIB_VERSION,
            "stamp must match STDLIB_VERSION"
        );
    });
}

#[test]
fn fresh_extraction_matches_embedded_content() {
    with_mvl_home(|_home| {
        let stdlib_dir = ensure_stdlib();

        for (name, embedded) in STDLIB_FILES {
            let on_disk = fs::read_to_string(stdlib_dir.join(name))
                .unwrap_or_else(|_| panic!("stdlib file {name} must be readable"));
            assert_eq!(
                on_disk, *embedded,
                "on-disk {name} must match embedded content"
            );
        }
    });
}

#[test]
fn second_call_is_idempotent() {
    with_mvl_home(|_home| {
        let stdlib_dir = ensure_stdlib();
        let stdlib_dir2 = ensure_stdlib();
        assert_eq!(stdlib_dir, stdlib_dir2, "path must be stable");

        // Compare file contents rather than mtime (mtime resolution is filesystem-dependent).
        for (name, embedded) in STDLIB_FILES {
            let on_disk = fs::read_to_string(stdlib_dir.join(name)).unwrap_or_else(|_| {
                panic!("stdlib file {name} must be readable after second call")
            });
            assert_eq!(on_disk, *embedded, "second call must not corrupt {name}");
        }
        let stamp = fs::read_to_string(stdlib_dir.join(".version")).expect(".version must exist");
        assert_eq!(
            stamp.trim(),
            STDLIB_VERSION,
            "stamp must be unchanged after second call"
        );
    });
}

/// Verify that `args.mvl` is present in the embedded STDLIB_FILES.
/// This guards against accidentally omitting it from the registry.
#[test]
fn args_mvl_is_in_stdlib_files() {
    assert!(
        STDLIB_FILES.iter().any(|(name, _)| *name == "args.mvl"),
        "args.mvl must be registered in STDLIB_FILES"
    );
}

/// A missing file with a valid version stamp triggers re-extraction.
///
/// This handles the case where a new stdlib module is added without bumping
/// the version number (e.g. a patch that adds `args.mvl` to an existing
/// `0.36.0` stdlib installation).
#[test]
fn missing_file_triggers_reextraction_despite_valid_stamp() {
    with_mvl_home(|_home| {
        // First extraction — all files present, stamp written.
        let stdlib_dir = ensure_stdlib();

        // Delete one stdlib file to simulate a partial / stale installation.
        let (first_file, first_content) = &STDLIB_FILES[0];
        let target = stdlib_dir.join(first_file);
        fs::remove_file(&target).expect("remove file");
        assert!(!target.exists(), "file must be gone before second call");

        // Second call — stamp is current but a file is missing → must re-extract.
        let _ = ensure_stdlib();

        let on_disk = fs::read_to_string(&target)
            .unwrap_or_else(|_| panic!("{first_file} must be re-extracted"));
        assert_eq!(
            on_disk, *first_content,
            "re-extracted {first_file} must match embedded content"
        );
    });
}

#[test]
fn stale_stamp_triggers_reextraction() {
    with_mvl_home(|home| {
        let std_dir = versioned_stdlib_path(home);
        fs::create_dir_all(&std_dir).expect("mkdir");
        fs::write(std_dir.join(".version"), "0.0.0-stale").expect("write stale stamp");

        let _ = ensure_stdlib();

        let stamp = fs::read_to_string(std_dir.join(".version")).expect("read stamp");
        assert_eq!(
            stamp.trim(),
            STDLIB_VERSION,
            "stamp must be updated to current version after re-extraction"
        );
        // Verify file contents were actually refreshed, not just that the file exists.
        for (name, embedded) in STDLIB_FILES {
            let on_disk = fs::read_to_string(std_dir.join(name)).unwrap_or_else(|_| {
                panic!("stdlib file {name} must be readable after re-extraction")
            });
            assert_eq!(
                on_disk, *embedded,
                "re-extracted {name} must match embedded content"
            );
        }
    });
}
