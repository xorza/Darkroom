//! The disk-backed blob store: the low-level "where does a node's outputs file live, and
//! read/write it" layer. Pure I/O — it holds the [`Library`] snapshot (its type table is the
//! source of custom-value codecs) and the optional content-addressed root, and knows *nothing*
//! about the [`RuntimeCache`](crate::execution::cache::RuntimeCache) that owns it and drives
//! the reuse/eviction policy. A disk-backed (`Disk`/`Both`) node's outputs live at
//! `<disk_root>/<hex(digest)>`, keyed by its content digest. Reproducible: any upstream
//! change re-keys it (so it's auto-invalidated), and identical computations dedup across
//! nodes/machines. A present blob is the same bytes, so [`store`](DiskStore::store) skips it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::data::{DataType, DynamicValue};
use crate::execution::blob;
use crate::execution::digest::Digest;
use crate::execution::program::{ExecutionProgram, NodeIdx};
use crate::library::Library;
use crate::runtime::context::ContextManager;

/// Reads and writes node-output blobs on disk. Holds a snapshot of the [`Library`] (its type
/// table is the source of custom-value codecs) and the optional content-addressed store root.
/// Default is an empty library ⇒ no codecs, disk-backed nodes memory-only.
///
/// The snapshot is fine to hold across library swaps: codecs are registered once at assembly
/// and never change (only subgraphs grow), so a held snapshot keeps every codec it will ever
/// need.
#[derive(Debug, Default)]
pub struct DiskStore {
    library: Arc<Library>,
    /// Root of the content-addressed store; `None` ⇒ disk-backed nodes memory-only.
    disk_root: Option<PathBuf>,
}

impl DiskStore {
    /// Build a store over `library` (its type table supplies the custom-value codecs) and an
    /// optional content-addressed store `disk_root` (`None` ⇒ `persist` nodes are
    /// memory-only).
    pub fn new(library: Arc<Library>, disk_root: Option<PathBuf>) -> Self {
        Self { library, disk_root }
    }

    /// The file node `idx` caches to — a disk-backed (`persists_to_disk`) node with a disk
    /// root and a content `digest` — or `None` when it doesn't cache to disk. Takes the
    /// digest rather than reading it off the cache, so this layer stays free of
    /// `RuntimeCache`.
    pub(crate) fn blob_path(
        &self,
        program: &ExecutionProgram,
        idx: NodeIdx,
        digest: Option<Digest>,
    ) -> Option<PathBuf> {
        let e_node = &program.e_nodes[idx];
        if !e_node.cache.persists_to_disk() {
            return None;
        }
        let digest = digest?;
        let mut buf = [0u8; 64];
        Some(self.disk_root.as_ref()?.join(hex(&digest, &mut buf)))
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

    /// Deserialize the blob at `path` into node outputs, or `None` if it can't be read/decoded
    /// (corrupt, an incompatible format, a missing codec, or gone).
    pub(crate) async fn read(&self, path: &Path) -> Option<Vec<DynamicValue>> {
        blob::read(path, &self.library).await
    }

    /// Delete a blob — used to clear a file that failed to decode, so the recompute that
    /// follows writes a fresh one rather than [`store`](Self::store) skipping the broken file
    /// as "already on disk" forever. Best-effort.
    pub(crate) fn delete(&self, path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    /// Serialize `outputs` to `path`. A blob already on disk is the same bytes
    /// (content-addressed) → skipped, avoiding a redundant, possibly costly serialize. The
    /// outputs are **borrowed**, not cloned — the write future captures only the value slice
    /// (which is `Sync`), never the whole (non-`Sync`) cache, so the borrow can safely cross
    /// the serialize await. Best-effort: any failure is logged — caching never fails a run.
    pub(crate) async fn store(
        &self,
        path: &Path,
        outputs: &[DynamicValue],
        ctx: &mut ContextManager,
    ) {
        if path.exists() {
            return;
        }
        if let Err(e) = blob::write(path, outputs, &self.library, ctx).await {
            tracing::warn!(path = %path.display(), error = %e, "failed to write output cache");
        }
    }
}

/// Lowercase hex of a digest into a caller-owned stack buffer (no heap) — the
/// 64-char content-addressed blob filename. The returned `&str` borrows `buf`, so the
/// caller keeps it alive until the path is built. Avoids a per-call `String`; the
/// remaining `PathBuf` for the `stat`/read is inherent and dwarfed by that syscall.
fn hex<'a>(digest: &Digest, buf: &'a mut [u8; 64]) -> &'a str {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (i, &byte) in digest.0.iter().enumerate() {
        buf[2 * i] = HEX[(byte >> 4) as usize];
        buf[2 * i + 1] = HEX[(byte & 0x0f) as usize];
    }
    std::str::from_utf8(buf).expect("hex digits are ASCII")
}

#[cfg(test)]
mod tests;
