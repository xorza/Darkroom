//! What a flush sweep did not leave on disk.

use crate::exec::cache::runtime_cache::error::{CacheFlushUnsupported, CacheNodeFailure};

/// What a flush sweep did *not* leave on disk. Split in two because the caller decides which half
/// to raise: a failure is a fault of this write and may pass on the next one, while an
/// unsupported type will never persist until the library registers a codec for it.
///
/// Absent from both vectors is the node with nothing to write: not disk-backed, or holding no
/// value current under its digest — the ordinary answer for a node that has not run.
#[derive(Debug, Default)]
pub(crate) struct CacheFlushReport {
    pub(crate) failures: Vec<CacheNodeFailure>,
    pub(crate) unsupported: Vec<CacheFlushUnsupported>,
}
