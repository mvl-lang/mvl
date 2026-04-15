use mvl::mvl::stdlib::{ensure_stdlib, STDLIB_FILES, STDLIB_VERSION};
use std::fs;
use std::sync::{LazyLock, Mutex};

// Serialize all tests that mutate the MVL_HOME env var, since env vars are
// process-global and test threads run concurrently by default.
static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Set MVL_HOME to a temp dir, run the closure, then clear the env var.
/// Acquires ENV_LOCK to prevent concurrent mutation of MVL_HOME.
fn with_mvl_home<F: FnOnce(&std::path::Path)>(f: F) {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().expect("tempdir");
    // MVL_HOME points at the data root; ensure_stdlib() appends "std"
    std::env::set_var("MVL_HOME", tmp.path());
    f(tmp.path());
    std::env::remove_var("MVL_HOME");
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

        let mtime_before = fs::metadata(stdlib_dir.join("core.mvl"))
            .expect("core.mvl must exist")
            .modified()
            .expect("mtime");

        let stdlib_dir2 = ensure_stdlib();
        assert_eq!(stdlib_dir, stdlib_dir2, "path must be stable");

        let mtime_after = fs::metadata(stdlib_dir.join("core.mvl"))
            .expect("core.mvl must exist")
            .modified()
            .expect("mtime");

        assert_eq!(
            mtime_before, mtime_after,
            "second call must not overwrite files when version matches"
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
        assert!(
            std_dir.join("core.mvl").exists(),
            "core.mvl must be present"
        );
    });
}
