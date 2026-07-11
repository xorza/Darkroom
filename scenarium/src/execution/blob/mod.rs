//! The on-disk storage core of the output cache: turn a node's `Vec<DynamicValue>`
//! into a file and back, through the codec registry, written atomically. A blob file
//! is `[content digest — 32 bytes][codec frame]`: the digest is the value's identity
//! key, checked on every presence test and read, so a file overwritten by a newer
//! configuration can never serve stale bytes. The policy layers — how a key maps to
//! a path, presence, eviction — live above this module.

use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;

use crate::data::DynamicValue;
use crate::execution::codec::{self, deserialize_outputs, serialize_outputs};
use crate::execution::digest::Digest;
use crate::library::Library;
use crate::runtime::context::ContextManager;

/// A real failure while storing outputs (a skipped non-cacheable value is *not* an
/// error — see [`write`]'s `Ok(false)`).
#[derive(Debug, Error)]
pub(crate) enum Error {
    #[error("encoding outputs for the cache failed: {0}")]
    Encode(#[from] codec::Error),
    #[error("writing a cache blob failed: {0}")]
    Write(#[from] io::Error),
}

/// The content digest stamped in a blob's leading 32 bytes, or `None` when the file
/// is absent, unreadable, or too short to carry one. The cheap presence/validity
/// probe: comparing it against a node's current digest answers "would [`read`] with
/// that digest hit?" without touching the body.
pub(crate) fn stored_digest(path: &Path) -> Option<Digest> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; 32];
    file.read_exact(&mut buf).ok()?;
    Some(Digest(buf))
}

/// Read + decode the outputs at `path`, or `None` on any miss: the file is absent,
/// unreadable, carries a digest other than `digest` (a superseded write), or no
/// longer decodes (corrupt / a custom type whose codec is gone). All mean
/// "recompute" to the caller. A transient read error is logged but, like the rest,
/// treated as a miss.
///
/// Mirrors [`write`]: the fs read + decode of a possibly huge blob runs on the
/// blocking pool so it doesn't stall the async worker thread (progress events,
/// cancel polling, other event-loop tasks).
pub(crate) async fn read(
    path: &Path,
    digest: Digest,
    library: &Arc<Library>,
) -> Option<Vec<DynamicValue>> {
    let path = path.to_path_buf();
    let library = library.clone();
    tokio::task::spawn_blocking(move || read_blocking(&path, digest, &library))
        .await
        .expect("cache read task panicked")
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

/// Serialize `outputs` and atomically write them to `path`, stamped with `digest`.
/// Returns whether a blob was actually written: `Ok(false)` when an output has no
/// registered codec (the node isn't cacheable — a silent skip, not a failure),
/// `Ok(true)` on a write. `Err` is a real encode or write failure.
pub(crate) async fn write(
    path: &Path,
    digest: Digest,
    outputs: &[DynamicValue],
    library: &Library,
    ctx: &mut ContextManager,
) -> Result<bool, Error> {
    let bytes = match serialize_outputs(outputs, library, ctx).await {
        Ok(bytes) => bytes,
        Err(codec::Error::UnknownType(_)) => return Ok(false),
        Err(e) => return Err(Error::Encode(e)),
    };
    // `atomic_write` is blocking `std::fs`; run it off the async worker thread so a
    // large blob's write doesn't stall the runtime (progress events, cancel polling,
    // other event-loop tasks). `serialize_outputs` above already did the heavy encode.
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || atomic_write(&path, &digest.0, &bytes))
        .await
        .expect("cache write task panicked")?;
    Ok(true)
}

/// Write `header` then `body` to `path` via a sibling temp file + `rename` (atomic
/// on one filesystem), creating the parent dir — so a reader never sees a
/// half-written blob and a crash mid-write can't corrupt an existing one. The two
/// slices are written back-to-back rather than concatenated, sparing a copy of a
/// possibly huge body. A rename that fails because a concurrent writer already
/// landed the target is tolerated (best-effort, last-writer-wins cache; the digest
/// header keeps readers honest).
fn atomic_write(path: &Path, header: &[u8], body: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = temp_path(path);
    let write_tmp = || -> io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(header)?;
        file.write_all(body)
    };
    write_tmp()?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            if path.exists() { Ok(()) } else { Err(e) }
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
