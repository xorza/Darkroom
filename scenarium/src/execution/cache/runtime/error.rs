//! What the cache's maintenance sweeps report back.

use std::fmt;

use crate::data::type_system::TypeId;
use crate::execution::cache::disk_store::error::{RemovalError, StoreError};
use crate::graph::identity::NodeId;

/// One node a sweep could not do its work on, named by the id the host asked
/// in.
///
/// Not a `Result`: a sweep covers many nodes and a failure on one says nothing
/// about the rest, so the whole sweep runs and hands back what it could not do.
///
/// Shared by eviction and flush. What the sweep was *trying* to do is already
/// said by the report it arrives in, so the two need no separate types.
#[derive(Debug)]
pub struct CacheNodeFailure {
    pub node_id: NodeId,
    pub cause: CacheNodeError,
}

impl fmt::Display for CacheNodeFailure {
    /// One entry of the `details` list a worker cache error carries.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.node_id, self.cause)
    }
}

/// Why a sweep could not finish with one node — the store operation it asked
/// for, still carrying its own cause rather than a rendering of it.
#[derive(Debug, thiserror::Error)]
pub enum CacheNodeError {
    #[error(transparent)]
    Removal(RemovalError),
    #[error(transparent)]
    Store(StoreError),
}

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

/// One node holding a value whose custom type has no registered codec, so the
/// flush wrote nothing for it and no future flush will either.
#[derive(Debug)]
pub struct CacheFlushUnsupported {
    pub node_id: NodeId,
    pub type_id: TypeId,
}

impl fmt::Display for CacheFlushUnsupported {
    /// One entry of the `details` list, beside [`CacheNodeFailure`]'s.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?}: no cache codec registered for type {:?}",
            self.node_id, self.type_id
        )
    }
}
