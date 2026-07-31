use std::path::{Path, PathBuf};

use crate::internals;

/// A temporary *path*, removed when the value drops — whether the test left a
/// file there, a directory, or nothing at all.
///
/// Unlike [`TempDir`](crate::internals::temp_dir::TempDir) nothing is created
/// up front: the point is a name for something the code under test will write,
/// and several fixtures turn that name into a directory precisely to make the
/// write fail.
#[derive(Debug)]
pub struct TempFile(PathBuf);

impl TempFile {
    /// A path nothing occupies. `tag` names the fixture asking.
    pub fn new(tag: &str) -> Self {
        Self(internals::unique_temp_path(tag))
    }

    /// [`new`](Self::new) with a suffix, for code that reads the extension.
    pub fn with_extension(tag: &str, extension: &str) -> Self {
        Self(internals::unique_temp_path(tag).with_extension(extension))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn exists(&self) -> bool {
        self.0.exists()
    }

    /// The path as a string, for the fixtures that bind one as a literal.
    pub fn to_str(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if std::fs::remove_file(&self.0).is_err() {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }
}
