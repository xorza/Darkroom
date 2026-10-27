//! [`TempDir`]: a directory of the process's own, removed with what it holds.

use std::fs;
use std::io;
use std::iter;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

/// A fresh, empty directory, removed with everything under it when the value drops.
///
/// Holding the directory in a value ties its lifetime to its owner's: an owner that made its own
/// path had to remember to clean up on every exit, the panicking one included.
#[derive(Debug)]
pub struct TempDir(PathBuf);

impl TempDir {
    /// A fresh empty directory in `parent`. `tag` names what it is for, so a directory a killed
    /// process left says what it was.
    pub fn create_in(parent: &Path, tag: &str) -> io::Result<Self> {
        create_first_free(iter::repeat_with(|| candidate(parent, tag))).map(Self)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, name: impl AsRef<Path>) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Best effort: a test that made an entry unremovable on purpose, to watch an eviction
        // fail on it, must not then fail in teardown.
        fs::remove_dir_all(&self.0).ok();
    }
}

/// The next name in `parent` for `tag`: the process id separates processes, and the counter the
/// names one process asks for. [`TempDir`] and the test `TempFile` share it, so the two cannot
/// pick one name.
pub(crate) fn candidate(parent: &Path, tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!("darkroom-{tag}-{}-{sequence}", process::id()))
}

/// Make the first of `candidates` nothing occupies, not even a dangling link: a killed process
/// leaves its directories behind, and a later process can have its id. The make is the test, so
/// two makers cannot both take one name.
fn create_first_free(candidates: impl IntoIterator<Item = PathBuf>) -> io::Result<PathBuf> {
    for candidate in candidates {
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    unreachable!("an endless supply of candidates")
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use std::fs;
    use std::path::PathBuf;

    use crate::TempDir;
    use crate::internals::temp_root;

    impl TempDir {
        /// A fresh empty directory for a test, on disk. `tag` names the fixture asking.
        pub fn new(tag: &str) -> Self {
            Self::create_in(temp_root(), tag).expect("a test temp directory is creatable")
        }

        /// The entries directly under it — how a fixture counts what a store wrote without
        /// naming any of it.
        pub fn entry_count(&self) -> usize {
            fs::read_dir(&self.0)
                .expect("a live temp directory lists")
                .flatten()
                .count()
        }

        /// The entries directly under it, in a stable order.
        pub fn entries(&self) -> Vec<PathBuf> {
            let mut entries: Vec<PathBuf> = fs::read_dir(&self.0)
                .expect("a live temp directory lists")
                .flatten()
                .map(|entry| entry.path())
                .collect();
            entries.sort();
            entries
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileSystem, fs as common_fs};

    /// A directory lies in its parent, named for the workspace and its tag, goes with its drop,
    /// and skips every name a file, a directory or a dangling link holds.
    #[test]
    fn a_temp_dir_skips_taken_names_and_goes_with_its_drop() {
        let parent = TempDir::new("parent");
        assert_eq!(
            parent.path().parent().unwrap(),
            FileSystem::disk_temp_dir().unwrap()
        );
        let dir = TempDir::create_in(parent.path(), "probe").unwrap();
        assert_eq!(dir.path().parent().unwrap(), parent.path());
        let name = dir.path().file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("darkroom-probe-"), "{name}");
        assert!(common_fs::is_dir(dir.path()));
        let path = dir.path().to_path_buf();
        drop(dir);
        assert!(!common_fs::exists(&path), "the drop removes it");

        let taken = ["file", "dir", "link"].map(|name| parent.join(name));
        fs::write(&taken[0], b"").unwrap();
        fs::create_dir(&taken[1]).unwrap();
        common_fs::symlink(parent.join("absent"), &taken[2]).unwrap();
        let free = parent.join("free");
        assert_eq!(
            create_first_free(taken.into_iter().chain([free.clone()])).unwrap(),
            free
        );
        assert!(common_fs::is_dir(&free));
    }
}
