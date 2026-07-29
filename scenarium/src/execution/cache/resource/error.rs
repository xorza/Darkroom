//! Why a filesystem path could not be identified.
//!
//! Stamping walks real paths, so this is genuine I/O failure or a run that was
//! cancelled mid-walk — recoverable, and attributed to the one node whose input
//! declared the path rather than aborting the run.

use std::io;

/// Why a path has no identity.
///
/// Nothing here is an [`FsPathId`](crate::execution::cache::resource::FsPathId) variant, absence included. An error
/// dressed as a value is exactly what let "I could not see this" fold
/// into a digest as if it were an identity — and a stable one, so the
/// node kept reusing a cached result while what it could not see changed
/// underneath it. A path that is not there is the same answer reached by
/// another road: a node whose declared input does not exist has nothing
/// to be a function of, so it fails at its own turn with the path in the
/// message, rather than being handed an identity and invoked to discover
/// as much for itself.
#[derive(Debug, thiserror::Error)]
pub(crate) enum StampError {
    /// The path would not read: one that is not there, a directory that
    /// would not list, an entry that would not stat, a file with no
    /// modification time.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The run was cancelled mid-walk.
    #[error("the run was cancelled")]
    Cancelled,
}
