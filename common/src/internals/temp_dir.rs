use std::path::{Path, PathBuf};

use crate::internals;

/// A temporary directory, created empty and removed with everything under it
/// when the value drops.
///
/// Holding the directory in a value is what ties its lifetime to the test's: a
/// fixture that made its own path had to remember to clean up on every exit,
/// including the panicking one.
#[derive(Debug)]
pub struct TempDir(PathBuf);

impl TempDir {
    /// A fresh empty directory. `tag` names the fixture asking, so a directory
    /// left behind by a killed run says what it was for.
    pub fn new(tag: &str) -> Self {
        let path = internals::unique_temp_path(tag);
        std::fs::create_dir_all(&path).expect("a test temp directory is creatable");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, name: impl AsRef<Path>) -> PathBuf {
        self.0.join(name)
    }

    /// The entries directly under it — how a fixture counts what a store wrote
    /// without naming any of it.
    pub fn entry_count(&self) -> usize {
        std::fs::read_dir(&self.0)
            .expect("a live temp directory lists")
            .flatten()
            .count()
    }

    /// The entries directly under it, in a stable order.
    pub fn entries(&self) -> Vec<PathBuf> {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&self.0)
            .expect("a live temp directory lists")
            .flatten()
            .map(|entry| entry.path())
            .collect();
        entries.sort();
        entries
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Best effort: a test that deliberately made an entry unremovable — to
        // watch an eviction fail on it — must not then fail in teardown.
        std::fs::remove_dir_all(&self.0).ok();
    }
}
