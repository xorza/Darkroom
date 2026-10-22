//! [`CachedSource`]: a source file as the decode cache names and keys it.

use std::path::{Path, PathBuf};

use common::{FileIdentity, fs};

use crate::decode::decoder_kind::DecoderKind;
use crate::frame_store::decode_cache::cache_key::CacheKey;
use crate::frame_store::error::FrameStoreError;
use crate::io::load_ctx::LoadCtx;

/// A source file as the decode cache names and keys it: the path it was read by, its canonical
/// path, and the key of its decode as the file was then.
#[derive(Debug)]
pub(crate) struct CachedSource {
    pub(crate) path: PathBuf,
    pub(crate) canonical: PathBuf,
    pub(crate) key: CacheKey,
}

impl CachedSource {
    /// `path` as it is now, decoded by `decoder` under `ctx`.
    pub(crate) fn of(
        path: &Path,
        decoder: DecoderKind,
        ctx: &LoadCtx,
    ) -> Result<Self, FrameStoreError> {
        let canonical = canonicalize(path)?;
        let identity = identity(path, &canonical)?;
        Ok(Self {
            path: path.to_path_buf(),
            canonical,
            key: CacheKey::new(identity, decoder, ctx),
        })
    }

    /// The source read again by its path, refused when it changed since [`Self::of`] read it: its
    /// decode would otherwise be kept under the identity it had before.
    ///
    /// # Errors
    ///
    /// [`FrameStoreError::SourceChanged`], or the source cannot be read.
    pub(super) fn ensure_unchanged(&self) -> Result<(), FrameStoreError> {
        let now = identity(&self.path, &canonicalize(&self.path)?)?;
        if now == self.key.source {
            Ok(())
        } else {
            Err(FrameStoreError::SourceChanged {
                path: self.path.clone(),
            })
        }
    }
}

/// `path` canonicalized, a failure named by `path`.
fn canonicalize(path: &Path) -> Result<PathBuf, FrameStoreError> {
    fs::canonicalize(path).map_err(|source| FrameStoreError::ReadMetadata {
        path: path.to_path_buf(),
        source,
    })
}

/// The identity of `canonical`, the canonical form of `path`, a failure named by `path`.
fn identity(path: &Path, canonical: &Path) -> Result<FileIdentity, FrameStoreError> {
    FileIdentity::of(canonical).map_err(|source| FrameStoreError::ReadMetadata {
        path: path.to_path_buf(),
        source,
    })
}
