//! The on-disk storage core shared by both output caches: turn a node's
//! `Vec<DynamicValue>` into a file and back, through the codec registry, written
//! atomically. The caches differ only in *policy* — how a key maps to a path (a
//! content digest under a root vs. an explicit path) and their presence/eviction
//! — not in how a value becomes bytes on disk, which lives here once.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;

use crate::context::ContextManager;
use crate::data::DynamicValue;
use crate::library::Library;
use crate::value_codec::{self, deserialize_outputs, serialize_outputs};

/// A real failure while storing outputs (a skipped non-cacheable value is *not* an
/// error — see [`write`]'s `Ok(false)`).
#[derive(Debug, Error)]
pub(crate) enum Error {
    #[error("encoding outputs for the cache failed: {0}")]
    Encode(#[from] value_codec::Error),
    #[error("writing a cache blob failed: {0}")]
    Write(#[from] io::Error),
}

/// Read + decode the outputs at `path`, or `None` on any miss: the file is absent,
/// unreadable, or no longer decodes (corrupt / a custom type whose codec is gone).
/// All mean "recompute" to the caller. A transient read error is logged but, like
/// the rest, treated as a miss.
pub(crate) fn read(path: &Path, func_lib: &Library) -> Option<Vec<DynamicValue>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "cache read failed; treating as miss");
            return None;
        }
    };
    match deserialize_outputs(bytes, func_lib) {
        Ok(values) => Some(values),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "cached outputs failed to decode; recomputing");
            None
        }
    }
}

/// Serialize `outputs` and atomically write them to `path`. Returns whether a blob
/// was actually written: `Ok(false)` when an output has no registered codec (the
/// node isn't cacheable — a silent skip, not a failure), `Ok(true)` on a write.
/// `Err` is a real encode or write failure.
pub(crate) async fn write(
    path: &Path,
    outputs: &[DynamicValue],
    func_lib: &Library,
    ctx: &mut ContextManager,
) -> Result<bool, Error> {
    let bytes = match serialize_outputs(outputs, func_lib, ctx).await {
        Ok(bytes) => bytes,
        Err(value_codec::Error::UnknownType(_)) => return Ok(false),
        Err(e) => return Err(Error::Encode(e)),
    };
    atomic_write(path, &bytes)?;
    Ok(true)
}

/// Write `bytes` to `path` via a sibling temp file + `rename` (atomic on one
/// filesystem), creating the parent dir — so a reader never sees a half-written
/// blob and a crash mid-write can't corrupt an existing one. A rename that fails
/// because a concurrent writer already landed the target is tolerated (best-effort
/// cache: content-addressed → same bytes; explicit path → last-writer-wins).
fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = temp_path(path);
    std::fs::write(&tmp, bytes)?;
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
mod tests {
    use super::*;
    use crate::data::{CustomValue, StaticValue, TypeDef};
    use std::any::Any;
    use std::fmt;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    /// A unique temp file path removed on drop.
    struct TempFile(PathBuf);
    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn temp_file(tag: &str) -> TempFile {
        static C: AtomicU64 = AtomicU64::new(0);
        let n = C.fetch_add(1, Ordering::Relaxed);
        TempFile(std::env::temp_dir().join(format!(
            "scenarium-blob-{tag}-{}-{n}.bin",
            std::process::id()
        )))
    }

    fn func_lib() -> Library {
        Library::default()
    }

    #[tokio::test]
    async fn write_then_read_round_trips_outputs() {
        let file = temp_file("roundtrip");
        let outputs = vec![
            DynamicValue::Unbound,
            DynamicValue::Static(StaticValue::Int(7)),
            DynamicValue::Static(StaticValue::String("x".into())),
        ];
        let wrote = write(
            &file.0,
            &outputs,
            &func_lib(),
            &mut ContextManager::default(),
        )
        .await
        .unwrap();
        assert!(wrote, "plain values are written");

        let back = read(&file.0, &func_lib()).expect("hit");
        assert_eq!(back.len(), 3);
        assert!(matches!(back[0], DynamicValue::Unbound));
        assert_eq!(back[1].as_i64(), Some(7));
        assert_eq!(back[2].as_string(), Some("x"));
    }

    #[test]
    fn read_missing_is_none() {
        let file = temp_file("missing");
        assert!(read(&file.0, &func_lib()).is_none());
    }

    /// A custom value with no registered codec — never cacheable.
    #[derive(Debug)]
    struct Opaque;
    impl fmt::Display for Opaque {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Opaque")
        }
    }
    impl CustomValue for Opaque {
        fn type_def(&self) -> Arc<TypeDef> {
            Arc::new(TypeDef {
                type_id: "78391861-24da-4368-a3a5-2a6b7a47f112".into(),
                display_name: "Opaque".into(),
            })
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[tokio::test]
    async fn non_codecable_custom_is_skipped_not_written() {
        let file = temp_file("noncodec");
        let outputs = vec![
            DynamicValue::Static(StaticValue::Int(1)),
            DynamicValue::from_custom(Opaque),
        ];
        let wrote = write(
            &file.0,
            &outputs,
            &func_lib(),
            &mut ContextManager::default(),
        )
        .await
        .unwrap();
        assert!(!wrote, "no codec ⇒ Ok(false), nothing written");
        assert!(!file.0.exists(), "no blob created");
    }
}
