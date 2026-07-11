//! The disk-backed blob store: the low-level "where does a node's outputs file live, and
//! read/write it" layer. Pure I/O — it holds the [`Library`] snapshot (its type table is the
//! source of custom-value codecs) and the optional store root, and knows *nothing* about the
//! [`RuntimeCache`](crate::execution::cache::RuntimeCache) that owns it and drives the
//! reuse/eviction policy. A disk-backed (`Disk`/`Both`) node's outputs live at
//! `<disk_root>/<hex(node id)>` — **one blob per node** — with the content digest they were
//! produced under as the file's first 32 bytes. A digest change overwrites the node's blob in
//! place, so a superseded configuration's bytes never linger as an orphan; the header keeps it
//! correct — a blob carrying a digest other than the node's current one reads as a miss, never
//! a stale hit.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::data::{DataType, DynamicValue};
use crate::execution::blob;
use crate::execution::digest::Digest;
use crate::execution::program::{ExecutionProgram, NodeIdx};
use crate::library::Library;
use crate::runtime::context::ContextManager;

/// Reads and writes node-output blobs on disk. Holds a snapshot of the [`Library`] (its type
/// table is the source of custom-value codecs) and the optional store root. Default is an
/// empty library ⇒ no codecs, disk-backed nodes memory-only.
///
/// The snapshot is fine to hold across library swaps: codecs are registered once at assembly
/// and never change (only subgraphs grow), so a held snapshot keeps every codec it will ever
/// need.
#[derive(Debug, Default)]
pub struct DiskStore {
    library: Arc<Library>,
    /// Root of the blob store; `None` ⇒ disk-backed nodes memory-only.
    disk_root: Option<PathBuf>,
}

/// Where one node's outputs cache on disk, paired with the content digest the blob
/// must carry to be valid — every store operation needs both halves together.
/// Produced by [`DiskStore::blob_target`].
#[derive(Debug)]
pub(crate) struct BlobTarget {
    pub(crate) path: PathBuf,
    pub(crate) digest: Digest,
}

impl DiskStore {
    /// Build a store over `library` (its type table supplies the custom-value codecs) and an
    /// optional store `disk_root` (`None` ⇒ `persist` nodes are memory-only).
    pub fn new(library: Arc<Library>, disk_root: Option<PathBuf>) -> Self {
        Self { library, disk_root }
    }

    /// The file node `idx` caches to — keyed by its **node id**, so the node has exactly one
    /// blob and a recompute under a new digest replaces the old one — plus the content
    /// `digest` a valid blob there must carry. `None` when the node doesn't cache to disk: not
    /// `persists_to_disk`, no disk root, or no digest (an impure cone). Takes the digest
    /// rather than reading it off the cache, so this layer stays free of `RuntimeCache`.
    pub(crate) fn blob_target(
        &self,
        program: &ExecutionProgram,
        idx: NodeIdx,
        digest: Option<Digest>,
    ) -> Option<BlobTarget> {
        let e_node = &program.e_nodes[idx];
        if !e_node.cache.persists_to_disk() {
            return None;
        }
        let digest = digest?;
        let mut buf = [0u8; 32];
        let name = e_node.id.as_uuid().simple().encode_lower(&mut buf);
        Some(BlobTarget {
            path: self.disk_root.as_ref()?.join(name),
            digest,
        })
    }

    /// Whether a blob stamped with `target.digest` sits at `target.path` — the "would a read
    /// hit?" probe, answered from the 32-byte header without touching the body. A file
    /// carrying another digest is a superseded write, not a hit.
    pub(crate) fn has_current_blob(&self, target: &BlobTarget) -> bool {
        blob::stored_digest(&target.path) == Some(target.digest)
    }

    /// Whether every one of `types` could be decoded back from a blob: each `Custom` output
    /// type has a codec in this store's library. `types` are a node's resolved output types off
    /// the program pool, so this predicts (without reading) whether [`read`](Self::read) would
    /// succeed — the reuse policy never flags a node whose later on-demand load would fail. An
    /// unresolved type (`Any`) imposes no constraint.
    pub(crate) fn outputs_decodable(&self, types: &[DataType]) -> bool {
        types.iter().all(|ty| match ty {
            DataType::Custom(type_id) => self.library.codec(type_id).is_some(),
            _ => true,
        })
    }

    /// Deserialize the blob at `target` into node outputs, or `None` if it can't be served
    /// (carries another digest, corrupt, an incompatible format, a missing codec, or gone).
    pub(crate) async fn read(&self, target: &BlobTarget) -> Option<Vec<DynamicValue>> {
        blob::read(&target.path, target.digest, &self.library).await
    }

    /// Delete a blob — used to clear a file that failed to decode, so the recompute that
    /// follows writes a fresh one rather than [`store`](Self::store) skipping the broken file
    /// as "already current" forever. Best-effort.
    pub(crate) fn delete(&self, path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    /// Serialize `outputs` to `target`, stamped with its digest. A blob already carrying that
    /// digest is the same bytes → skipped, avoiding a redundant, possibly costly serialize; a
    /// blob under any *other* digest is superseded and overwritten — this is where a stale
    /// cache dies instead of orphaning. The outputs are **borrowed**, not cloned — the write
    /// future captures only the value slice (which is `Sync`), never the whole (non-`Sync`)
    /// cache, so the borrow can safely cross the serialize await. Best-effort: any failure is
    /// logged — caching never fails a run.
    pub(crate) async fn store(
        &self,
        target: &BlobTarget,
        outputs: &[DynamicValue],
        ctx: &mut ContextManager,
    ) {
        if self.has_current_blob(target) {
            return;
        }
        if let Err(e) = blob::write(&target.path, target.digest, outputs, &self.library, ctx).await
        {
            tracing::warn!(path = %target.path.display(), error = %e, "failed to write output cache");
        }
    }
}

#[cfg(test)]
mod tests;
