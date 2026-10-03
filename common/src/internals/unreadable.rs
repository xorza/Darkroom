//! [`Unreadable`]: a file or directory with its permissions taken away for a test.

use std::fs::{self, File, Permissions};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

/// A path with mode `000` for as long as the guard lives; dropping it restores the mode the
/// path had, on a panic too, so a fixture never outlives its test unremovable.
#[derive(Debug)]
pub struct Unreadable {
    path: PathBuf,
    original: Permissions,
}

impl Unreadable {
    /// Take every permission away from `path`. `None` when the process bypasses permission
    /// checks (root, `CAP_DAC_OVERRIDE`), so no path can be unreadable to it: the mode is
    /// restored at once, and the test that wanted the fixture says so and skips.
    #[expect(
        clippy::print_stderr,
        reason = "a test that skips says so on the test output"
    )]
    pub fn new(path: &Path) -> Option<Self> {
        let original = fs::metadata(path)
            .expect("an unreadable fixture starts from an existing path")
            .permissions();
        fs::set_permissions(path, Permissions::from_mode(0o000))
            .expect("take a fixture's permissions away");
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
                "skipping: this process reads {} at mode 000 (it bypasses permission checks)",
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
