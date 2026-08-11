//! How the store's two write-side operations fail.

use std::io;
use std::path::PathBuf;

use crate::data::codec::error::CodecFormatError;
use crate::execution::cache::disk_store::store_outcome::StoreOutcome;

/// A publication that was attempted and did not land, named by the stage that
/// broke so a report says which half of the write failed.
///
/// Every arm is I/O or an external codec's own failure — expected, and
/// recoverable by degrading: a cache is an optimization, so a caller drops the
/// blob rather than failing the run.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("could not create the cache directory for {}: {source}", path.display())]
    Directory {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not begin publishing {}: {source}", path.display())]
    Begin {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not encode the outputs for {}: {source}", path.display())]
    Encode {
        path: PathBuf,
        #[source]
        source: CodecFormatError,
    },
    #[error("could not write {}: {source}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not publish {}: {source}", path.display())]
    Publish {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub(crate) type StoreResult = std::result::Result<StoreOutcome, StoreError>;

/// A blob that would not go away. A blob that was never there is not one of
/// these — an eviction wants the file gone, and an absent file already is.
#[derive(Debug, thiserror::Error)]
#[error("failed to remove {}: {source}", path.display())]
pub struct RemovalError {
    pub path: PathBuf,
    #[source]
    pub source: io::Error,
}
