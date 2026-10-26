//! Test-only helpers, reachable from downstream crates' test targets under the
//! `internals` feature without entering the released surface.

pub(crate) mod temp_dir;
pub(crate) mod temp_file;
#[cfg(unix)]
pub(crate) mod unreadable;

use std::env;
use std::fs;
use std::iter;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::FileSystem;

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

/// The publication temporaries beside `destination`: every file named like the ones
/// [`fs`](crate::fs) writes before it renames, which a publication that failed
/// must not leave behind.
pub fn publication_temp_files(destination: &Path) -> Vec<PathBuf> {
    let prefix = format!("{}.", destination.file_name().unwrap().to_string_lossy());
    fs::read_dir(destination.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|candidate| {
            candidate
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&prefix)
                && candidate
                    .extension()
                    .is_some_and(|extension| extension == "tmp")
        })
        .collect()
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("`common` sits under the workspace root")
        .to_path_buf()
}

/// A free path in [`FileSystem::disk_temp_dir`] nothing else will pick: `tag` says which fixture
/// wants it, the process id separates concurrent test binaries, and the counter separates repeated
/// calls within one. On disk, not on the tmpfs `/tmp` of Debian 13 and Arch: a frame-sized fixture
/// there spends RAM, and lumos's frame store refuses to spill to a memory-backed directory at all.
/// Straight in the temporary directory, without a root of its own, which a second user of a shared
/// `/var/tmp` could not write into.
///
/// Shared by [`TempDir`](temp_dir::TempDir) and [`TempFile`](temp_file::TempFile),
/// so the two cannot collide with each other either.
fn unique_temp_path(tag: &str) -> PathBuf {
    static ROOT: LazyLock<PathBuf> = LazyLock::new(|| {
        FileSystem::disk_temp_dir().expect("a temporary directory on disk: set `TMPDIR` to one")
    });
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    first_free(iter::repeat_with(|| {
        let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
        ROOT.join(format!("darkroom-{tag}-{}-{sequence}", process::id()))
    }))
}

/// The first of `candidates` nothing occupies, not even a dangling link: a killed run leaves its
/// paths behind, and a later process can have its id.
fn first_free(candidates: impl IntoIterator<Item = PathBuf>) -> PathBuf {
    candidates
        .into_iter()
        .find(|candidate| fs::symlink_metadata(candidate).is_err())
        .expect("an endless supply of candidates")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TempDir, fs};

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
    }

    /// A temp path lies on disk, named for the workspace and its fixture, and skips every name a
    /// file, a directory or a dangling link holds.
    #[test]
    fn temp_paths_lie_on_disk_and_skip_taken_names() {
        let dir = TempDir::new("probe");
        assert_eq!(
            dir.path().parent().unwrap(),
            FileSystem::disk_temp_dir().unwrap()
        );
        let name = dir.path().file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("darkroom-probe-"), "{name}");
        assert_eq!(FileSystem::memory_backed(dir.path()), None);

        let taken = ["file", "dir", "link"].map(|name| dir.join(name));
        fs::write(&taken[0], b"").unwrap();
        fs::create_dir(&taken[1]).unwrap();
        fs::symlink(dir.join("absent"), &taken[2]).unwrap();
        let free = dir.join("free");
        assert_eq!(first_free(taken.into_iter().chain([free.clone()])), free);
    }
}
