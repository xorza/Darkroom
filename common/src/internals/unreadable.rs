//! [`Unreadable`]: a file or directory with its permissions taken away for a test.

use std::fs::{self, File, Permissions};
use std::path::{Path, PathBuf};

use crate::platform;

/// A path with every permission taken away for as long as the guard lives; dropping it restores
/// the permissions the path had, on a panic too, so a fixture never outlives its test
/// unremovable.
#[derive(Debug)]
pub struct Unreadable {
    path: PathBuf,
    original: Permissions,
}

impl Unreadable {
    /// Take every permission away from `path`: mode `000` on Unix. `None` where the system has no
    /// such fixture, as Windows, and when the process bypasses permission checks (root,
    /// `CAP_DAC_OVERRIDE`), so no path can be unreadable to it: the permissions are restored at
    /// once, and the test that wanted the fixture says so and skips.
    #[expect(
        clippy::print_stderr,
        reason = "a test that skips says so on the test output"
    )]
    pub fn new(path: &Path) -> Option<Self> {
        let Some(original) = platform::internals::take_permissions(path) else {
            eprintln!(
                "skipping: this system has no unreadable fixture for {}",
                path.display()
            );
            return None;
        };
        let guard = Self {
            path: path.to_path_buf(),
            original,
        };
        let still_readable = if path.is_dir() {
            fs::read_dir(path).is_ok()
        } else {
            File::open(path).is_ok()
        };
        if still_readable {
            eprintln!(
                "skipping: this process reads {} with its permissions taken away (it bypasses \
                 permission checks)",
                path.display()
            );
            return None;
        }
        Some(guard)
    }
}

impl Drop for Unreadable {
    fn drop(&mut self) {
        fs::set_permissions(&self.path, self.original.clone())
            .expect("restore a fixture's permissions");
    }
}
