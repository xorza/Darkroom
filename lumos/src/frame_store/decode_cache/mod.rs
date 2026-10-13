//! [`DecodeCache`]: the decoded frames a `keep_cache` run keeps for the next.

pub(crate) mod cache_key;
pub(crate) mod cached_source;
pub(crate) mod decode_options;
pub(crate) mod decode_pins;
pub(crate) mod fnv1a;
pub(crate) mod frame_spill;

use std::path::Path;

use crate::frame::frame_stats::FrameStats;
use crate::frame::stackable_image::StackableImage;
use crate::frame_store::decode_cache::cached_source::CachedSource;
use crate::frame_store::decode_cache::frame_spill::FrameSpill;
use crate::frame_store::disk_root::DiskRoot;
use crate::frame_store::error::{FrameStoreError, KeptDecodeError};
use crate::frame_store::stored_frame::{ReusedFrame, StoredFrame};
use crate::io::load_context::LoadContext;

/// The directory under the cache root a kept frame lives in.
const DIRECTORY: &str = "decode-cache";

/// Decoded frames keyed by their source and decoder, kept across runs: what `keep_cache` writes and
/// a later run reuses. Only content-keyed frames go here, each committed under its
/// [`CacheKey`](crate::frame_store::decode_cache::cache_key::CacheKey) (see
/// [`FrameSpill::cached`]); what one run alone needs goes to its
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

    /// The files the frame of `source` is kept in.
    fn spill<'a>(&'a self, source: &'a CachedSource) -> FrameSpill<'a> {
        FrameSpill::cached(self.path(), &source.canonical, source.key)
    }

    /// The frame committed for `source` as it is now; `None` when none whole is
    /// ([`StoredFrame::reuse`]).
    pub(crate) fn reuse(
        &self,
        source: &CachedSource,
    ) -> Result<Option<ReusedFrame>, FrameStoreError> {
        StoredFrame::reuse(&self.spill(source))
    }

    /// Keep `image`, decoded from `source` after it was read, with its `stats`. The source is read
    /// again first, and one that changed while it was decoded is refused rather than kept under
    /// the identity it had before.
    ///
    /// # Errors
    ///
    /// [`FrameStoreError::SourceChanged`], or the files cannot be written.
    pub(crate) fn keep(
        &self,
        source: &CachedSource,
        image: &impl StackableImage,
        stats: FrameStats,
    ) -> Result<StoredFrame, FrameStoreError> {
        source.ensure_unchanged()?;
        StoredFrame::cache(&self.spill(source), image, stats)
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
        let source = CachedSource::of(path, I::DECODER, context)?;
        if let Some(reused) = self.reuse(&source)? {
            return Ok(reused.into_image());
        }
        let image = I::load(path, context)?;
        self.keep(&source, &image, FrameStats::measure(&image))?;
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
            .filter(
                |path| match CachedSource::of(path.as_ref(), I::DECODER, context) {
                    Ok(source) => self.spill(&source).committed().is_none(),
                    Err(_) => true,
                },
            )
            .count()
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use std::fs;
    use std::path::Path;

    use crate::frame::stackable_image::StackableImage;
    use crate::frame_store::decode_cache::DecodeCache;
    use crate::frame_store::decode_cache::cached_source::CachedSource;
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
            let source = CachedSource::of(path, I::DECODER, context).unwrap();
            let spill = self.spill(&source);
            let channels = spill
                .committed()
                .expect("a frame is kept for the path")
                .decoded
                .dimensions
                .channels();
            let plane_path = spill.channel_path(0);
            let mut plane: Vec<f32> =
                bytemuck::cast_slice(&fs::read(&plane_path).unwrap()).to_vec();
            edit(&mut plane);
            fs::write(&plane_path, bytemuck::cast_slice(&plane)).unwrap();
            spill.reseal(channels);
        }
    }
}
