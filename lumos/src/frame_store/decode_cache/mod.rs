//! [`DecodeCache`]: the decoded frames a `keep_cache` run keeps for the next.

pub(crate) mod cache_key;
pub(crate) mod cached_source;
pub(crate) mod frame_spill;
pub(crate) mod kept_frame;

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use crate::frame::admitted::Admitted;
use crate::frame_store::decode_cache::cached_source::CachedSource;
use crate::frame_store::decode_cache::frame_spill::{Committed, FrameSpill};
use crate::frame_store::decode_cache::kept_frame::KeptFrame;
use crate::frame_store::disk_root::DiskRoot;
use crate::frame_store::error::FrameStoreError;
use crate::image::flat_gain::FlatGain;
use crate::image::flat_gain::gain_key::GainKey;
use crate::image::stackable_image::StackableImage;

/// The directory under the cache root a kept frame lives in.
const DIR: &str = "decode-cache";

/// Decoded frames keyed by their source and decoder, kept across runs: what `keep_cache` writes and
/// a later run reuses. Only content-keyed frames go here, each committed under its
/// [`CacheKey`](crate::frame_store::decode_cache::cache_key::CacheKey) (see
/// [`FrameSpill::cached`]); what one run alone needs goes to its
/// [`RunScratch`](crate::frame_store::run_scratch::RunScratch). Lumos never removes it.
#[derive(Debug)]
pub(crate) struct DecodeCache {
    root: DiskRoot,
    /// The keys of the gains this run wrote: each written once in a run, so one a run before left
    /// damaged is replaced, and every later frame of it writes none.
    gains: Mutex<Vec<GainKey>>,
}

impl DecodeCache {
    /// The cache under `root`, created if missing.
    pub(crate) fn open(root: &Path) -> Result<Self, FrameStoreError> {
        Ok(Self {
            root: DiskRoot::create(&root.join(DIR))?,
            gains: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        self.root.path()
    }

    /// See [`DiskRoot::ensure_space`].
    pub(super) fn ensure_space(&self, needed: u64) -> Result<(), FrameStoreError> {
        self.root.ensure_space(needed)
    }

    /// The files the frame of `source` is kept in.
    pub(super) fn spill<'a>(&'a self, source: &'a CachedSource) -> FrameSpill<'a> {
        FrameSpill::cached(self.path(), &source.canonical, source.key)
    }

    /// What the cache committed for `source` as it is now; `None` when it holds no frame of it.
    pub(super) fn committed(&self, source: &CachedSource) -> Option<Committed> {
        self.spill(source).committed()
    }

    /// Keep `image`, decoded from `source` after it was read, as its admission found it: its gain
    /// first, once in a run for each key, then its planes and the commit. The source is read again
    /// first, and one that changed while it was decoded is refused rather than kept under the
    /// identity it had before.
    ///
    /// # Errors
    ///
    /// [`FrameStoreError::SourceChanged`], or the files cannot be written.
    pub(crate) fn keep(
        &self,
        source: &CachedSource,
        image: &impl StackableImage,
        admitted: Admitted,
    ) -> Result<KeptFrame, FrameStoreError> {
        source.ensure_unchanged()?;
        let spill = self.spill(source);
        if let Some(gain) = image.flat_gain() {
            self.keep_gain(&spill, gain)?;
        }
        KeptFrame::keep(&spill, image, admitted)
    }

    /// Write `gain` to its file, unless this run wrote it already.
    fn keep_gain(&self, spill: &FrameSpill<'_>, gain: &FlatGain) -> Result<(), FrameStoreError> {
        let key = gain.key();
        let mut written = self.gains.lock().unwrap_or_else(PoisonError::into_inner);
        if written.contains(&key) {
            return Ok(());
        }
        spill.publish_gain(gain)?;
        written.push(key);
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use std::path::Path;

    use common::fs;

    use crate::frame_store::decode_cache::DecodeCache;
    use crate::frame_store::decode_cache::cached_source::CachedSource;
    use crate::frame_store::decode_cache::kept_frame::KeptFrame;
    use crate::image::flat_gain::shared_gains::SharedGains;
    use crate::io::decoder::Decoder;
    use crate::io::load_ctx::LoadCtx;

    impl DecodeCache {
        /// The frame kept for `path` decoded by `D` under `ctx`, as the image it was decoded to;
        /// `None` when none is.
        pub(crate) fn kept_image<D: Decoder>(
            &self,
            path: &Path,
            ctx: &LoadCtx,
        ) -> Option<D::Image> {
            let source = CachedSource::of(path, D::KIND, ctx).ok()?;
            KeptFrame::reuse_image(
                &self.spill(&source),
                self.committed(&source)?,
                &SharedGains::default(),
            )
            .map(|kept| kept.image)
        }

        /// Edit channel 0 of the frame kept for `path` decoded by `D` under `ctx`, as anything
        /// outside lumos could, and commit it again as it now is: a test that edits a kept frame on
        /// purpose sees a later run read it back rather than decode the source.
        pub(crate) fn edit_kept<D: Decoder>(
            &self,
            path: &Path,
            ctx: &LoadCtx,
            edit: impl FnOnce(&mut [f32]),
        ) {
            let source = CachedSource::of(path, D::KIND, ctx).unwrap();
            let spill = self.spill(&source);
            assert!(spill.committed().is_some(), "a frame is kept for the path");
            let plane_path = spill.channel_path(0);
            let mut plane: Vec<f32> =
                bytemuck::cast_slice(&fs::read(&plane_path).unwrap()).to_vec();
            edit(&mut plane);
            fs::write(&plane_path, bytemuck::cast_slice(&plane)).unwrap();
            spill.reseal();
        }
    }
}
