//! Test-only helpers, reachable from downstream crates' test targets under the
//! `internals` feature without entering the released surface.

pub(crate) mod temp_dir;
pub(crate) mod temp_file;
#[cfg(unix)]
pub(crate) mod unreadable;

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

/// The environment variable that turns debug output on: any value does.
pub const DEBUG_OUTPUT_VAR: &str = "DARKROOM_TEST_OUTPUT";

/// Where a test may write a file for a person to look at — `test_output/<name>` under the
/// workspace root, with its parent directories made — or `None` when nobody asked: the output
/// is opt-in through [`DEBUG_OUTPUT_VAR`], so an ordinary run writes nothing.
pub fn debug_output_path(name: &str) -> Option<PathBuf> {
    env::var_os(DEBUG_OUTPUT_VAR)?;
    // `common` sits one level under the workspace root.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let path = root.join("test_output").join(name);
    fs::create_dir_all(path.parent().expect("a debug file has a parent directory"))
        .expect("create a debug output directory");
    Some(path)
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
    env::temp_dir().join(format!("{tag}-{}-{sequence}", process::id()))
}
