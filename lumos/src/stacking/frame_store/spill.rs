//! Where a spilled frame's planes live on disk, and what they are called.
//!
//! Every name the frame store writes comes from [`FrameSpill`], so the writer that produced a
//! plane and a later run looking for it cannot disagree about where it is. The files themselves
//! are not tracked individually: they all sit inside a [`SpillDirectory`], which removes the whole
//! directory on drop unless `keep_cache` asked otherwise.

use std::mem::size_of;
use std::path::{Path, PathBuf};

use arrayvec::ArrayVec;
use common::file_utils;

use crate::io::image::image_dimensions::ImageDimensions;
use crate::stacking::frame_store::error::FrameStoreError;
use crate::stacking::frame_store::{StackableImage, StoredPlane};

/// Owns a spill directory and removes it after its mapped frames have dropped.
#[derive(Debug)]
pub(crate) struct SpillDirectory {
    pub(crate) path: PathBuf,
    keep: bool,
}

impl SpillDirectory {
    pub(crate) fn create(path: PathBuf, keep: bool) -> Result<Self, FrameStoreError> {
        std::fs::create_dir_all(&path).map_err(|source| FrameStoreError::CreateDirectory {
            path: path.clone(),
            source,
        })?;
        Ok(Self { path, keep })
    }
}

impl Drop for SpillDirectory {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Write each channel under `spill` and memory-map it back.
///
/// The files are not tracked for removal here. Everything the frame store spills lives inside a
/// [`SpillDirectory`], which removes the directory wholesale on drop unless `keep_cache` asked for
/// it to survive — so a per-file drop guard would either duplicate that or, if it ignored the flag,
/// quietly delete what the user asked to keep.
pub(crate) fn spill_channels(
    spill: FrameSpill<'_>,
    image: &impl StackableImage,
) -> Result<ArrayVec<StoredPlane, 3>, FrameStoreError> {
    let mut planes = ArrayVec::new();
    for channel in 0..image.dimensions().channels() {
        let path = spill.channel_path(channel);
        write_plane(&path, image.channel(channel))?;
        planes.push(StoredPlane::map(path)?);
    }
    Ok(planes)
}

/// The files one frame occupies inside a spill directory: one plane per channel, plus the
/// optional warp-quality planes.
///
/// Every name the frame store writes comes from here, so the writer that produced a plane and a
/// later run looking for it cannot disagree about where it lives.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameSpill<'a> {
    directory: &'a Path,
    name: &'a str,
}

impl<'a> FrameSpill<'a> {
    pub(crate) fn new(directory: &'a Path, name: &'a str) -> Self {
        Self { directory, name }
    }

    /// Basename for caching `source` across runs: a hash, so distinct sources never collide and
    /// the same source always resolves to the same files.
    pub(crate) fn cache_name(source: &Path) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"lumos-frame-cache-path-v1\0");
        hasher.update(source.as_os_str().as_encoded_bytes());
        format!("{}.bin", hasher.finalize().to_hex())
    }

    /// `cache_name` hands back a `.bin` basename; every file below appends its own suffix, so the
    /// extension is stripped once here rather than doubled onto each name.
    fn stem(self) -> &'a str {
        self.name.strip_suffix(".bin").unwrap_or(self.name)
    }

    pub(crate) fn channel_path(self, channel: usize) -> PathBuf {
        self.directory
            .join(format!("{}_c{channel}.bin", self.stem()))
    }

    /// Path of a warp-quality plane — `coverage` or `confidence`.
    pub(crate) fn quality_path(self, kind: &str) -> PathBuf {
        self.directory.join(format!("{}_{kind}.bin", self.stem()))
    }

    /// Whether every channel plane is already on disk at the size `dimensions` implies. A plane
    /// of the wrong length is a stale cache from different geometry, not a reusable one.
    pub(crate) fn channels_reusable(self, dimensions: ImageDimensions) -> bool {
        let expected = (dimensions.pixel_count() * size_of::<f32>()) as u64;
        (0..dimensions.channels()).all(|channel| {
            std::fs::metadata(self.channel_path(channel))
                .is_ok_and(|metadata| metadata.len() == expected)
        })
    }

    /// What this frame's cache holds for its quality planes.
    ///
    /// A frame that carries them writes both before the identity sidecar that commits the cache,
    /// so a valid one has both or neither. Checked rather than assumed: these are files, and
    /// anything can disturb them between runs.
    pub(crate) fn cached_quality(self, dimensions: ImageDimensions) -> CachedQuality {
        let expected = (dimensions.pixel_count() * size_of::<f32>()) as u64;
        let present = |kind| {
            std::fs::metadata(self.quality_path(kind)).is_ok_and(|meta| meta.len() == expected)
        };
        match (present("coverage"), present("confidence")) {
            (true, true) => CachedQuality::Present,
            (false, false) => CachedQuality::Absent,
            _ => CachedQuality::Torn,
        }
    }
}

/// What a cached frame's quality planes look like on disk.
///
/// Three states rather than a bool because the middle one has to be actionable: a frame that wrote
/// neither plane is reusable and carries none, one that wrote both is reusable and carries them,
/// and one holding a lone plane is neither — [`WarpQuality`](crate::stacking::frame_store::warp_quality::WarpQuality)
/// documents why a lone plane is not a shape any producer means, so the cache is rebuilt instead of
/// being read as either of the other two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CachedQuality {
    Absent,
    Present,
    Torn,
}

pub(crate) fn write_plane(path: &Path, pixels: &[f32]) -> Result<(), FrameStoreError> {
    let bytes = bytemuck::cast_slice(pixels);
    file_utils::publish_bytes(path, bytes, file_utils::PublicationMode::Cache).map_err(|source| {
        FrameStoreError::WriteFile {
            path: path.to_path_buf(),
            source,
        }
    })
}
