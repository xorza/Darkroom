//! A system neither Unix nor Windows: a rename replaces, no directory is synced, and nothing is
//! stated beyond length and modification time.

use std::fs::{self, Metadata};
use std::io;
use std::path::Path;

use crate::file_utils::PublicationMode;
use crate::file_utils::platform::{Observed, Stated};

pub(super) fn replace(source: &Path, destination: &Path, _mode: PublicationMode) -> io::Result<()> {
    fs::rename(source, destination)
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "one signature on every system: Unix syncs the directory, which can fail"
)]
pub(super) const fn sync_parent(_parent: &Path) -> io::Result<()> {
    Ok(())
}

pub(super) fn stated(_metadata: &Metadata) -> Stated {
    Stated::default()
}

pub(super) fn observe(path: &Path) -> io::Result<Observed> {
    Ok(Observed {
        metadata: fs::metadata(path)?,
        stated: Stated::default(),
    })
}
