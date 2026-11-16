//! What the cache's maintenance sweeps report back.

use std::fmt;

use common::ErrorChain;

use crate::data::data_type::TypeId;
use crate::exec::cache::disk_store::error::{RemovalError, StoreError};
use crate::graph::program_id::ProgramId;

/// One node a sweep could not do its work on, named by the id the host asked
/// in.
///
/// Not a `Result`: a sweep covers many nodes and a failure on one says nothing about the rest, so
/// the whole sweep runs and hands back what it could not do. Shared by eviction and flush; what
/// the sweep was *trying* to do is said by the report it arrives in.
#[derive(Debug)]
pub struct CacheNodeFailure {
    pub program_id: ProgramId,
    pub cause: CacheNodeError,
}

impl fmt::Display for CacheNodeFailure {
    /// One entry of the `details` list a worker cache error carries. The list holds one cause a
    /// node, which no single `source` can return, so each entry prints its cause's chain.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.program_id, ErrorChain::new(&self.cause))
    }
}

/// Why a sweep could not finish with one node — the store operation it asked
/// for, still carrying its own cause rather than a rendering of it.
#[derive(Debug, thiserror::Error)]
pub enum CacheNodeError {
    #[error(transparent)]
    Removal(#[from] RemovalError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// One node holding a value whose custom type has no registered codec, so the
/// flush wrote nothing for it and no future flush will either.
#[derive(Debug)]
pub struct CacheFlushUnsupported {
    pub program_id: ProgramId,
    pub type_id: TypeId,
}

impl fmt::Display for CacheFlushUnsupported {
    /// One entry of the `details` list, beside [`CacheNodeFailure`]'s.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?}: no cache codec registered for type {:?}",
            self.program_id, self.type_id
        )
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::PathBuf;

    use crate::exec::cache::disk_store::error::RemovalError;
    use crate::exec::cache::runtime_cache::error::{CacheNodeError, CacheNodeFailure};
    use crate::graph::identity::NodeId;
    use crate::graph::program_id::ProgramId;

    /// An entry of the list keeps what the OS said under the store's own words.
    #[test]
    fn a_failure_entry_prints_its_cause_chain() {
        let program_id = ProgramId::from(NodeId::unique());
        let failure = CacheNodeFailure {
            program_id,
            cause: CacheNodeError::from(RemovalError {
                path: PathBuf::from("/cache/blob"),
                source: io::Error::other("busy"),
            }),
        };
        assert_eq!(
            failure.to_string(),
            format!("{program_id:?}: failed to remove /cache/blob: busy")
        );
    }
}
