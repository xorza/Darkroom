//! Why a filesystem path could not be identified.
//!
//! Stamping walks real paths, so this is genuine I/O failure or a run that was
//! cancelled mid-walk — recoverable, and attributed to the one node whose input
//! declared the path rather than aborting the run.

use std::io;
use std::path::{Path, PathBuf};

/// Why a path has no identity.
///
/// Nothing here is an [`FsPathId`](crate::execution::cache::resource::FsPathId) variant, absence included. An error
/// dressed as a value is exactly what let "I could not see this" fold
/// into a digest as if it were an identity — and a stable one, so the
/// node kept reusing a cached result while what it could not see changed
/// underneath it. A path that is not there is the same answer reached by
/// another road: the node gets no digest and runs uncached, with a warning
/// naming the path, and its lambda reports for itself whether it needed
/// what was not there.
#[derive(Debug, thiserror::Error)]
pub(crate) enum StampError {
    /// The path would not read: one that is not there, a directory that
    /// would not list, an entry that would not stat, a file with no
    /// modification time. `path` is the one that failed, which inside a
    /// directory is the entry rather than the directory.
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The run was cancelled mid-walk.
    #[error("the run was cancelled")]
    Cancelled,
}

impl StampError {
    /// The [`Io`](Self::Io) error of `path`, for a `map_err`.
    pub(crate) fn io(path: &Path) -> impl FnOnce(io::Error) -> Self + '_ {
        move |source| Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}
