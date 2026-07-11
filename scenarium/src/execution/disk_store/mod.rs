//! The disk-backed blob store: where a node's outputs file lives, and turning
//! `Vec<DynamicValue>` into that file and back through the codec registry. Pure I/O — it
//! holds the [`Library`] snapshot (its type table is the source of custom-value codecs) and
//! the optional store root, and knows *nothing* about the
//! [`RuntimeCache`](crate::execution::cache::RuntimeCache) that owns it and drives the
//! reuse/eviction policy. A disk-backed (`Disk`/`Both`) node's outputs live at
//! `<disk_root>/<hex(node id)>` — **one blob per node** — as `[content digest — 32
//! bytes][codec frame]`, written atomically. A digest change overwrites the node's blob in
//! place, so a superseded configuration's bytes never linger as an orphan; the header keeps
//! it correct — every presence probe and read checks it, so a blob carrying a digest other
//! than the node's current one is a miss, never a stale hit.

use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::data::{DataType, DynamicValue};
use crate::execution::codec::{self, deserialize_outputs, serialize_outputs};
use crate::execution::digest::Digest;
use crate::execution::program::ExecutionNode;
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
/// Produced by [`DiskStore::blob_target`]. Carries the header/file-level operations
/// that need no codec table; anything decoding or encoding a body lives on
/// [`DiskStore`].
#[derive(Debug)]
pub(crate) struct BlobTarget {
    pub(crate) path: PathBuf,
    pub(crate) digest: Digest,
}

impl BlobTarget {
    /// Whether a blob stamped with this target's digest sits at its path — the "would a
    /// read hit?" probe, answered from the 32-byte header without touching the body. A
    /// file carrying another digest is a superseded write, not a hit.
    pub(crate) fn is_current(&self) -> bool {
        stored_digest(&self.path) == Some(self.digest)
    }

    /// Delete the blob — used to clear a file that failed to decode, so the recompute
    /// that follows writes a fresh one rather than [`DiskStore::store`] skipping the
    /// broken file as "already current" forever. Best-effort.
    pub(crate) fn delete(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl DiskStore {
    /// Build a store over `library` (its type table supplies the custom-value codecs) and an
    /// optional store `disk_root` (`None` ⇒ `persist` nodes are memory-only).
    pub fn new(library: Arc<Library>, disk_root: Option<PathBuf>) -> Self {
        Self { library, disk_root }
    }

    /// The file `e_node` caches to — keyed by its **node id**, so the node has exactly one
    /// blob and a recompute under a new digest replaces the old one — plus the content
    /// `digest` a valid blob there must carry. `None` when the node doesn't cache to disk: not
    /// `persists_to_disk`, no disk root, or no digest (an impure cone). Takes the digest
    /// rather than reading it off the cache, so this layer stays free of `RuntimeCache`.
    pub(crate) fn blob_target(
        &self,
        e_node: &ExecutionNode,
        digest: Option<Digest>,
    ) -> Option<BlobTarget> {
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

    /// Deserialize the blob at `target` into node outputs, or `None` on any miss: the file is
    /// absent, unreadable, carries a digest other than `target.digest` (a superseded write),
    /// or no longer decodes (corrupt / a custom type whose codec is gone). All mean
    /// "recompute" to the caller.
    ///
    /// Mirrors [`store`](Self::store): the fs read + decode of a possibly huge blob runs on
    /// the blocking pool so it doesn't stall the async worker thread (progress events, cancel
    /// polling, other event-loop tasks).
    pub(crate) async fn read(&self, target: &BlobTarget) -> Option<Vec<DynamicValue>> {
        let path = target.path.clone();
        let digest = target.digest;
        let library = self.library.clone();
        tokio::task::spawn_blocking(move || read_blocking(&path, digest, &library))
            .await
            .expect("cache read task panicked")
    }

    /// Serialize `outputs` to `target`, stamped with its digest. A blob already carrying that
    /// digest is the same bytes → skipped, avoiding a redundant, possibly costly serialize; a
    /// blob under any *other* digest is superseded and overwritten — this is where a stale
    /// cache dies instead of orphaning. An output whose custom type has no registered codec
    /// makes the node uncacheable — a silent skip, not a failure. The outputs are
    /// **borrowed**, not cloned — the write future captures only the value slice (which is
    /// `Sync`), never the whole (non-`Sync`) cache, so the borrow can safely cross the
    /// serialize await. Best-effort: any real failure is logged — caching never fails a run.
    pub(crate) async fn store(
        &self,
        target: &BlobTarget,
        outputs: &[DynamicValue],
        ctx: &mut ContextManager,
    ) {
        if target.is_current() {
            return;
        }
        let bytes = match serialize_outputs(outputs, &self.library, ctx).await {
            Ok(bytes) => bytes,
            Err(codec::Error::UnknownType(_)) => return,
            Err(e) => {
                tracing::warn!(path = %target.path.display(), error = %e, "failed to encode output cache");
                return;
            }
        };
        // `atomic_write` is blocking `std::fs`; run it off the async worker thread so a
        // large blob's write doesn't stall the runtime (progress events, cancel polling,
        // other event-loop tasks). `serialize_outputs` above already did the heavy encode.
        let path = target.path.clone();
        let digest = target.digest;
        let result = tokio::task::spawn_blocking(move || atomic_write(&path, digest, &bytes))
            .await
            .expect("cache write task panicked");
        if let Err(e) = result {
            tracing::warn!(path = %target.path.display(), error = %e, "failed to write output cache");
        }
    }
}

/// The content digest stamped in a blob's leading 32 bytes, or `None` when the file
/// is absent, unreadable, or too short to carry one. The cheap presence/validity
/// probe behind [`BlobTarget::is_current`], answered without touching the body.
fn stored_digest(path: &Path) -> Option<Digest> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; 32];
    file.read_exact(&mut buf).ok()?;
    Some(Digest(buf))
}

fn read_blocking(path: &Path, digest: Digest, library: &Library) -> Option<Vec<DynamicValue>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "cache read failed; treating as miss");
            return None;
        }
    };
    let Some(body) = bytes.strip_prefix(digest.0.as_slice()) else {
        // The presence check saw this digest, so the file changed underfoot (a
        // concurrent writer landed a newer configuration). Not an error — a miss.
        tracing::warn!(path = %path.display(), "cache blob carries a different digest; treating as miss");
        return None;
    };
    match deserialize_outputs(body, library) {
        Ok(values) => Some(values),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "cached outputs failed to decode; recomputing");
            None
        }
    }
}

/// Write the digest header then `body` to `path` via a sibling temp file + `rename`
/// (atomic on one filesystem), creating the parent dir — so a reader never sees a
/// half-written blob and a crash mid-write can't corrupt an existing one. The two
/// slices are written back-to-back rather than concatenated, sparing a copy of a
/// possibly huge body. No `sync_all` before the rename: a power loss may leave garbage
/// at the final path, but the digest/format checks turn that into a miss that
/// self-heals on the next run — cheaper than fsyncing every large blob. A rename lost
/// to a concurrent writer is tolerated only when the survivor carries **our** digest
/// (same bytes); a survivor under any other digest means this store failed and the
/// caller must hear about it.
fn atomic_write(path: &Path, digest: Digest, body: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = temp_path(path);
    let write_tmp = || -> io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&digest.0)?;
        file.write_all(body)
    };
    let result = write_tmp().and_then(|()| std::fs::rename(&tmp, path));
    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            // Don't leave the temp file behind — a disk-full store would otherwise
            // leak a fresh uniquely-named leftover on every failed attempt.
            let _ = std::fs::remove_file(&tmp);
            if stored_digest(path) == Some(digest) {
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}

/// A temp sibling path unique across processes and concurrent writes, so two
/// writers never share (and interleave into) one temp file.
fn temp_path(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.{n}.tmp", std::process::id()));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests;
