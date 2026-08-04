//! What one blob publication answers back.

use std::io;
use std::path::PathBuf;

use crate::data::codec;
use crate::data::type_system::TypeId;

/// What a publication that did not fail actually did.
///
/// All three arms mean "nothing further to do here"; they differ only in what
/// the *caller* owes the user, which is the caller's to decide. The run loop
/// stores after every invoke and reports none of them; a user-initiated flush
/// reports the ones that mean no blob exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreOutcome {
    /// A fresh blob is on disk.
    Published,
    /// [`StorePolicy::PreserveCovering`](super::StorePolicy::PreserveCovering)
    /// found a blob already covering the snapshot and left it untouched.
    AlreadyCovered,
    /// A value's custom type has no codec registered in the current library, so
    /// nothing was written.
    ///
    /// Not a failure: which codecs exist is a property of the library, not of
    /// this write, and the same value will go on not persisting until one is
    /// registered. That permanence is exactly why a caller reporting to a human
    /// wants it — nothing about retrying, or about the disk, will change it.
    Unsupported { type_id: TypeId },
}

/// A publication that was attempted and did not land, named by the stage that
/// broke so a report says which half of the write failed.
///
/// Every arm is I/O or an external codec's own failure — expected, and
/// recoverable by degrading: a cache is an optimization, so a caller drops the
/// blob rather than failing the run.
#[derive(Debug, thiserror::Error)]
pub(crate) enum StoreError {
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
        source: codec::error::Error,
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
