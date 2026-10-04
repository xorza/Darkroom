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

/// Where a test or example writes a file for a person to look at: `test_output/<name>` under the
/// workspace root, with its parent directories made.
pub fn output_path(name: &str) -> PathBuf {
    let path = workspace_root().join("test_output").join(name);
    fs::create_dir_all(
        path.parent()
            .expect("an output file has a parent directory"),
    )
    .expect("create an output directory");
    path
}

/// [`output_path`], or `None` when nobody asked: debug output is opt-in through
/// [`DEBUG_OUTPUT_VAR`], so an ordinary run writes nothing.
pub fn debug_output_path(name: &str) -> Option<PathBuf> {
    env::var_os(DEBUG_OUTPUT_VAR)?;
    Some(output_path(name))
}

/// A disk-backed scratch directory for a probe too large for the OS temp directory:
/// `.tmp/<name>` under the workspace root. Neither made nor removed here: the caller owns its
/// lifetime.
pub fn scratch_dir(name: &str) -> PathBuf {
    workspace_root().join(".tmp").join(name)
}

fn workspace_root() -> PathBuf {
    // `common` sits one level under the workspace root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("`common` sits under the workspace root")
        .to_path_buf()
}

/// A path under `.tmp/tests` in the workspace nothing else will pick: `tag` says which fixture
/// wants it, the process id separates concurrent test binaries, and the counter separates repeated
/// calls within one. On disk rather than in the OS temp directory, which Debian 13 and Arch mount
/// as tmpfs: a frame-sized fixture there spends RAM, and lumos's frame store refuses to spill to a
/// memory-backed directory at all.
///
/// Shared by [`TempDir`](temp_dir::TempDir) and [`TempFile`](temp_file::TempFile),
/// so the two cannot collide with each other either.
fn unique_temp_path(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = scratch_dir("tests");
    fs::create_dir_all(&root).expect("the test scratch root is creatable");
    root.join(format!("{tag}-{}-{sequence}", process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_resolve_under_the_workspace_root() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        assert!(
            root.join("Cargo.lock").is_file() && root.join("common/Cargo.toml").is_file(),
            "the root holds the lockfile and `common`"
        );
        let root = root.canonicalize().unwrap();

        let output = output_path("common_internals/probe.txt");
        assert!(output.parent().unwrap().is_dir(), "the parent is made");
        assert_eq!(
            output.parent().unwrap().canonicalize().unwrap(),
            root.join("test_output/common_internals")
        );
        assert_eq!(output.file_name().unwrap(), "probe.txt");
        fs::remove_dir(output.parent().unwrap()).unwrap();

        let scratch = scratch_dir("probe");
        assert!(scratch.ends_with(".tmp/probe"), "{}", scratch.display());
        assert_eq!(
            scratch
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .canonicalize()
                .unwrap(),
            root
        );
    }
}
