//! What a publication that did not fail actually did.

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
