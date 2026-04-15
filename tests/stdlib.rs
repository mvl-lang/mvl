use mvl::mvl::stdlib::{ensure_stdlib, STDLIB_FILES, STDLIB_VERSION};
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
    // MVL_HOME points at the data root; ensure_stdlib() appends "std"
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
            home.join("std"),
            "stdlib dir must be under MVL_HOME/std"
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

#[test]
fn stale_stamp_triggers_reextraction() {
    with_mvl_home(|home| {
        let std_dir = home.join("std");
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
