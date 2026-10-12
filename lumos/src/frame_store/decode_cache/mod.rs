//! [`DecodeCache`]: the decoded frames a `keep_cache` run keeps for the next.

pub(crate) mod cache_key;
pub(crate) mod cached_source;
pub(crate) mod frame_spill;

use std::path::Path;

use imaginarium::Buffer2;

use crate::frame::frame_quality::FrameQuality;
use crate::frame::frame_stats::FrameStats;
use crate::frame::stackable_image::StackableImage;
use crate::frame_store::decode_cache::cache_key::CacheKey;
use crate::frame_store::decode_cache::cached_source::CachedSource;
use crate::frame_store::decode_cache::frame_spill::FrameSpill;
use crate::frame_store::disk_root::DiskRoot;
use crate::frame_store::error::{FrameStoreError, KeptDecodeError};
use crate::frame_store::stored_frame::{ReusedFrame, StoredFrame};
use crate::io::load_context::LoadContext;

/// The directory under the cache root a kept frame lives in.
const DIRECTORY: &str = "decode-cache";

/// Decoded frames keyed by their source and decoder, kept across runs: what `keep_cache` writes and
/// a later run reuses. Only content-keyed frames go here, each committed under its [`CacheKey`]
/// (see [`FrameSpill::cached`]); what one run alone needs goes to its
/// [`RunScratch`](crate::frame_store::run_scratch::RunScratch). Lumos never removes it.
#[derive(Debug)]
pub(crate) struct DecodeCache {
    root: DiskRoot,
}

impl DecodeCache {
    /// The cache under `root`, created if missing.
    pub(crate) fn open(root: &Path) -> Result<Self, FrameStoreError> {
        Ok(Self {
            root: DiskRoot::create(&root.join(DIRECTORY))?,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        self.root.path()
    }

    /// See [`DiskRoot::ensure_space`].
    pub(crate) fn ensure_space(&self, needed: u64) -> Result<(), FrameStoreError> {
        self.root.ensure_space(needed)
    }

    /// The frame committed for `source` as it is now, decoded by `I` under `context`; `None` when
    /// none whole is ([`StoredFrame::reuse`]).
    pub(crate) fn reuse<I: StackableImage>(
        &self,
        source: &CachedSource,
        context: &LoadContext,
    ) -> Result<Option<ReusedFrame>, FrameStoreError> {
        let spill = FrameSpill::cached(self.path(), &source.canonical, I::DECODER);
        StoredFrame::reuse(&spill, CacheKey::new(source.identity, I::DECODER, context))
    }

    /// Keep `image`, which `I` decoded from `path` under `context` after `source` was read, with
    /// its `quality` and `stats`. The source is read again first, and one that changed while it
    /// was decoded is refused rather than kept under the identity it had before.
    ///
    /// # Errors
    ///
    /// [`FrameStoreError::SourceChanged`], or the files cannot be written.
    pub(crate) fn keep<I: StackableImage>(
        &self,
        path: &Path,
        source: &CachedSource,
        image: &I,
        quality: &FrameQuality<Buffer2<f32>>,
        stats: FrameStats,
        context: &LoadContext,
    ) -> Result<StoredFrame, FrameStoreError> {
        if CachedSource::of(path)?.identity != source.identity {
            return Err(FrameStoreError::SourceChanged {
                path: path.to_path_buf(),
            });
        }
        let spill = FrameSpill::cached(self.path(), &source.canonical, I::DECODER);
        let key = CacheKey::new(source.identity, I::DECODER, context);
        StoredFrame::cache(&spill, key, image, quality, stats)
    }

    /// `path` decoded by `I` under `context`, whole in RAM: read back from the cache when a frame
    /// is committed there for the source as it is now, decoded and kept otherwise. For an entry
    /// that changes a frame in place — calibration, a cosmic-ray pass — and measures it after.
    ///
    /// # Errors
    ///
    /// The decode, or the cache.
    pub(crate) fn decode<I: StackableImage>(
        &self,
        path: &Path,
        context: &LoadContext,
    ) -> Result<I, KeptDecodeError> {
        let source = CachedSource::of(path)?;
        if let Some(reused) = self.reuse::<I>(&source, context)? {
            return Ok(reused.into_image());
        }
        let image = I::load(path, context)?;
        let quality = FrameQuality::for_unwarped(&image);
        self.keep(
            path,
            &source,
            &image,
            &quality,
            FrameStats::measure(&image),
            context,
        )?;
        Ok(image)
    }

    /// How many of `paths` hold no frame committed for their source as it is now, decoded by `I`
    /// under `context`: the frames a run that reads them through the cache writes. A path that
    /// cannot be read counts as one, as its run fails on it anyway.
    pub(crate) fn missing<I: StackableImage>(
        &self,
        paths: &[impl AsRef<Path>],
        context: &LoadContext,
    ) -> usize {
        paths
            .iter()
            .filter(|path| match CachedSource::of(path.as_ref()) {
                Ok(source) => FrameSpill::cached(self.path(), &source.canonical, I::DECODER)
                    .committed(CacheKey::new(source.identity, I::DECODER, context))
                    .is_none(),
                Err(_) => true,
            })
            .count()
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use std::fs;
    use std::path::Path;

    use crate::frame::stackable_image::StackableImage;
    use crate::frame_store::decode_cache::DecodeCache;
    use crate::frame_store::decode_cache::cache_key::CacheKey;
    use crate::frame_store::decode_cache::cached_source::CachedSource;
    use crate::frame_store::decode_cache::frame_spill::FrameSpill;
    use crate::io::load_context::LoadContext;

    impl DecodeCache {
        /// Edit channel 0 of the frame kept for `path` decoded by `I` under `context`, as anything
        /// outside lumos could, and commit it again as it now is: a test that edits a kept frame on
        /// purpose sees a later run read it back rather than decode the source.
        pub(crate) fn edit_kept<I: StackableImage>(
            &self,
            path: &Path,
            context: &LoadContext,
            edit: impl FnOnce(&mut [f32]),
        ) {
            let source = CachedSource::of(path).unwrap();
            let spill = FrameSpill::cached(self.path(), &source.canonical, I::DECODER);
            let key = CacheKey::new(source.identity, I::DECODER, context);
            let channels = spill
                .committed(key)
                .expect("a frame is kept for the path")
                .decoded
                .dimensions
                .channels();
            let plane_path = spill.channel_path(0);
            let mut plane: Vec<f32> =
                bytemuck::cast_slice(&fs::read(&plane_path).unwrap()).to_vec();
            edit(&mut plane);
            fs::write(&plane_path, bytemuck::cast_slice(&plane)).unwrap();
            spill.reseal(key, channels);
        }
    }
}
