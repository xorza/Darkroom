//! Test-only helpers, reachable from downstream crates' test targets under the
//! `internals` feature without entering the released surface.

pub(crate) mod temp_dir;
pub(crate) mod temp_file;

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// Returns the workspace root directory.
/// Works by finding the directory containing Cargo.lock.
fn workspace_root() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).parent().unwrap().to_path_buf()
}

/// Ensures the test output directory exists. Safe to call multiple times.
fn ensure_test_output_dir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        std::fs::create_dir_all(workspace_root().join("test_output"))
            .expect("Failed to create test_output directory");
    });
}

/// Returns the path to a test output file.
/// Supports subdirectories - they will be created if needed.
pub fn test_output_path(name: &str) -> PathBuf {
    ensure_test_output_dir();
    let path = workspace_root().join("test_output").join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("Failed to create test output subdirectory");
    }
    path
}

/// A path under the OS temp directory nothing else will pick: `tag` says which
/// fixture wants it, the process id separates concurrent test binaries, and the
/// counter separates repeated calls within one.
///
/// Shared by [`TempDir`](temp_dir::TempDir) and [`TempFile`](temp_file::TempFile),
/// so the two cannot collide with each other either.
fn unique_temp_path(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{tag}-{}-{sequence}", std::process::id()))
}
