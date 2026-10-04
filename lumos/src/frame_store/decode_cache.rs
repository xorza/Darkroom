//! [`DecodeCache`]: the decoded frames a `keep_cache` run keeps for the next.

use std::path::Path;

use crate::frame_store::disk_root::DiskRoot;
use crate::frame_store::error::FrameStoreError;
use crate::mount_table::MountTable;

/// The directory under the cache root a kept frame lives in.
const DIRECTORY: &str = "decode-cache";

/// Decoded frames keyed by their source and decoder, kept across runs: what `keep_cache` writes
/// and a later run reuses. Only content-keyed frames go here, each committed under its
/// [`CacheKey`](crate::frame_store::cache_key::CacheKey) (see
/// [`FrameSpill::cached`](crate::frame_store::frame_spill::FrameSpill::cached)); what one run
/// alone needs goes to its [`RunScratch`](crate::frame_store::run_scratch::RunScratch). Lumos
/// never removes it.
#[derive(Debug)]
pub(crate) struct DecodeCache {
    root: DiskRoot,
}

impl DecodeCache {
    /// The cache under `root`, created if missing.
    pub(crate) fn open(root: &Path) -> Result<Self, FrameStoreError> {
        Ok(Self {
            root: DiskRoot::create(&root.join(DIRECTORY), &MountTable::read())?,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        self.root.path()
    }
}
