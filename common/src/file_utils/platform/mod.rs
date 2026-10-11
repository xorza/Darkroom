//! What differs between operating systems for a file: replacing it in one step, making a rename in
//! a directory durable, and what the system states of it beyond its length and modification time.
//!
//! Each system's side is a file of its own, picked here and nowhere else. A fact a system does not
//! state is a `None`, not a branch a caller has to know about.

#[cfg(not(any(unix, windows)))]
mod other;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(not(any(unix, windows)))]
use other as os;
#[cfg(unix)]
use unix as os;
#[cfg(windows)]
use windows as os;

use std::fs::Metadata;
use std::io;
use std::path::Path;

use crate::file_utils::PublicationMode;
use crate::file_utils::file_identity::FileId;

/// What the system states of a file beside its length and modification time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Stated {
    pub(super) file_id: Option<FileId>,
    pub(super) changed_ns: Option<i128>,
}

/// A file's metadata and what the system states of it beside, read together.
#[derive(Debug)]
pub(super) struct Observed {
    pub(super) metadata: Metadata,
    pub(super) stated: Stated,
}

/// Replace `destination` with `source` in one step: a reader sees the old file or the new one.
pub(super) fn replace(source: &Path, destination: &Path, mode: PublicationMode) -> io::Result<()> {
    os::replace(source, destination, mode)
}

/// Make the renames in `parent` durable, where the system needs a directory synced for that.
#[cfg_attr(
    not(unix),
    expect(
        clippy::missing_const_for_fn,
        reason = "const where no directory is synced, not on Unix"
    )
)]
pub(super) fn sync_parent(parent: &Path) -> io::Result<()> {
    os::sync_parent(parent)
}

/// What `metadata` states beside length and modification time, on a system whose metadata holds
/// it: Unix's does, Windows' does not.
pub(super) fn stated(metadata: &Metadata) -> Stated {
    os::stated(metadata)
}

/// The file `path` names, following a link, with everything the system states of it.
pub(super) fn observe(path: &Path) -> io::Result<Observed> {
    os::observe(path)
}
