//! [`StorageParams`]: where a run writes, and whether it keeps its decodes.

use std::path::PathBuf;

/// Where a run writes — its scratch, and its decode cache when it keeps one — as its config sets
/// it.
#[derive(Debug, Clone)]
pub(crate) struct StorageParams {
    /// See [`IngestConfig::cache_dir`](crate::IngestConfig::cache_dir).
    pub(crate) cache_dir: PathBuf,
    /// See [`IngestConfig::keep_cache`](crate::IngestConfig::keep_cache).
    pub(crate) keep_cache: bool,
}
