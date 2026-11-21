//! What a publication that did not fail actually did.

use crate::data::data_type::custom_type::CustomTypeId;

/// What a publication that did not fail actually did.
///
/// Every arm means "nothing further to do here"; they differ only in what the *caller* owes
/// the user. The run loop stores after every invoke and reports none of them; a user-initiated
/// flush reports the ones that mean no blob exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreOutcome {
    /// A fresh blob is on disk.
    Published,
    /// [`StorePolicy::PreserveCovering`](crate::exec::cache::disk_store::StorePolicy::PreserveCovering)
    /// found a blob already covering the snapshot and left it untouched.
    AlreadyCovered,
    /// A value's custom type has no codec registered in the current library, so nothing was
    /// written. Not a failure: which codecs exist is a property of the library, not of this
    /// write, and the value will go on not persisting until one is registered — which is why a
    /// caller reporting to a human wants it.
    Unsupported { type_id: CustomTypeId },
    /// Nothing was written, as nothing was owed: the node is not disk-backed, holds no value
    /// current under its key, or its last write failed.
    NothingOwed,
}
