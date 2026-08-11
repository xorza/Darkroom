//! What a flush sweep did not leave on disk.

use crate::execution::cache::runtime::error::{CacheFlushUnsupported, CacheNodeFailure};

/// What a flush sweep did *not* leave on disk. Split in two because the halves
/// are not equally worth a human's attention and the caller decides which to
/// raise: a failure is a fault of this write and may pass on the next one,
/// while an unsupported type will never persist until the library registers a
/// codec for it.
///
/// Silent here — and so absent from both vectors — is the node with nothing to
/// write: not disk-backed, or holding no value current under its digest. That
/// is the ordinary answer for a node that has not run, not a shortfall.
#[derive(Debug, Default)]
pub(crate) struct CacheFlushReport {
    pub(crate) failures: Vec<CacheNodeFailure>,
    pub(crate) unsupported: Vec<CacheFlushUnsupported>,
}
