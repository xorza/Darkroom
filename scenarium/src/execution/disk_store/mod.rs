//! The disk-backed blob store: the low-level "where does a node's outputs file live, and
//! read/write it" layer. Pure I/O — it holds the [`Library`] snapshot (its type table is the
//! source of custom-value codecs) and the optional content-addressed root, and knows *nothing*
//! about the [`RuntimeCache`](crate::execution::cache::RuntimeCache) that owns it and drives
//! the reuse/eviction policy. Two addressing schemes over the one storage primitive
//! ([`blob`]), differing only in how a node maps to a file and whether a present blob is
//! rewritten:
//!
//! - **content-addressed** — a disk-backed (`Disk`/`Both`) node's outputs at
//!   `<disk_root>/<hex(digest)>`, keyed by its content digest. Reproducible: any upstream
//!   change re-keys it (so it's auto-invalidated), and identical computations dedup across
//!   nodes/machines. A present blob is the same bytes, so [`store`](DiskStore::store) skips it.
//! - **explicit-path** — a [`CachePassthrough`](crate::node::special::SpecialNode) node's
//!   outputs at the `Const` `FsPath` in `input[1]`. The path *is* the key; the user manages
//!   invalidation (delete the file, or the `bypass` toggle). Always (over)written. See
//!   `README.md` Part C.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::data::{DataType, DynamicValue};
use crate::execution::blob;
use crate::execution::cache_node::cache_node_path;
use crate::execution::digest::Digest;
use crate::execution::program::{ExecutionProgram, NodeIdx};
use crate::library::Library;
use crate::node::special::SpecialNode;
use crate::runtime::context::ContextManager;

/// Where a node's outputs file lives, and how a present blob is treated on [`store`](DiskStore::store).
#[derive(Debug)]
pub(crate) enum Target {
    /// `<disk_root>/<hex(digest)>` — content-addressed: a present blob is the same
    /// bytes, so the store skips it (avoiding a redundant, possibly costly serialize).
    Addressed(PathBuf),
    /// An explicit path from `input[1]` — the store always (over)writes (a
    /// bypass/miss run rewrites).
    Explicit(PathBuf),
}

impl Target {
    pub(crate) fn path(&self) -> &Path {
        match self {
            Target::Addressed(p) | Target::Explicit(p) => p,
        }
    }
}

/// Reads and writes node-output blobs on disk. Holds a snapshot of the [`Library`] (its type
/// table is the source of custom-value codecs, used by both addressing schemes) and the
/// optional content-addressed store root. Default is an empty library ⇒ no codecs, disk-backed
/// nodes memory-only; `CachePassthrough` nodes always use their explicit path.
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
    /// optional content-addressed store `disk_root` (`None` ⇒ `persist` nodes are memory-only;
    /// `CachePassthrough` nodes always use their explicit path).
    pub fn new(library: Arc<Library>, disk_root: Option<PathBuf>) -> Self {
        Self { library, disk_root }
    }

    /// The file node `idx` caches to, or `None` when it doesn't: a `CachePassthrough` with a
    /// `Const` path → explicit; a disk-backed (`persists_to_disk`) node with a disk root and a
    /// content `digest` → content-addressed; anything else → none. Takes the digest rather than
    /// reading it off the cache, so this layer stays free of `RuntimeCache`.
    pub(crate) fn blob_path(
        &self,
        program: &ExecutionProgram,
        idx: NodeIdx,
        digest: Option<Digest>,
    ) -> Option<Target> {
        let e_node = &program.e_nodes[idx];
        if matches!(e_node.special, Some(SpecialNode::CachePassthrough { .. })) {
            return cache_node_path(program.node_inputs(e_node))
                .map(|p| Target::Explicit(PathBuf::from(p)));
        }
        if e_node.cache.persists_to_disk() {
            let digest = digest?;
            let mut buf = [0u8; 64];
            return Some(Target::Addressed(
                self.disk_root.as_ref()?.join(hex(&digest, &mut buf)),
            ));
        }
        None
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
    pub(crate) fn read(&self, path: &Path) -> Option<Vec<DynamicValue>> {
        blob::read(path, &self.library)
    }

    /// Delete a blob — used to clear a file that failed to decode, so the recompute that
    /// follows writes a fresh one rather than [`store`](Self::store) skipping the broken file
    /// as "already on disk" forever. Best-effort.
    pub(crate) fn delete(&self, path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    /// Serialize `outputs` to `target`'s file. A content-addressed (`Addressed`) blob already
    /// on disk is the same bytes → skipped; an `Explicit` path is always (over)written. The
    /// outputs are **borrowed**, not cloned — the write future captures only the value slice
    /// (which is `Sync`), never the whole (non-`Sync`) cache, so the borrow can safely cross
    /// the serialize await. Best-effort: any failure is logged — caching never fails a run.
    pub(crate) async fn store(
        &self,
        target: &Target,
        outputs: &[DynamicValue],
        ctx: &mut ContextManager,
    ) {
        if matches!(target, Target::Addressed(path) if path.exists()) {
            return;
        }
        if let Err(e) = blob::write(target.path(), outputs, &self.library, ctx).await {
            tracing::warn!(path = %target.path().display(), error = %e, "failed to write output cache");
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
