//! Memory planning and RAM/mmap storage shared by stacking stages.

use std::fs::File;
use std::mem::size_of;
use std::path::{Path, PathBuf};

use arrayvec::ArrayVec;
use imaginarium::Buffer2;
use memmap2::Mmap;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use common::file_utils;

use crate::io::image::error::ImageError;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::math::statistics::{MedianMad, mad_f32_with_scratch, median_f32_mut};

/// Failure while creating or accessing disk-backed frame storage.
#[derive(Debug, thiserror::Error)]
pub enum FrameStoreError {
    #[error("failed to create frame-store directory '{path}': {source}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write frame-store file '{path}': {source}")]
    WriteFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to open frame-store file '{path}': {source}")]
    OpenFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read metadata for frame-store source '{path}': {source}")]
    ReadMetadata {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("frame-store source changed while it was being read: '{path}'")]
    SourceChanged { path: PathBuf },
    #[error("failed to memory-map frame-store file '{path}': {source}")]
    MemoryMap {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

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

/// Per-frame statistics: one median/MAD pair per channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FrameStats {
    pub(crate) channels: ArrayVec<MedianMad, 3>,
    pub(crate) quantization_sigma: Option<f32>,
}

impl FrameStats {
    /// Measure per-channel median and MAD on `image`, before any interpolation touches it.
    pub(crate) fn measure(image: &impl StackableImage) -> Self {
        let dimensions = image.dimensions();
        let quantization_sigma = image.quantization_sigma();
        if dimensions.channels() == 1 {
            let data = image.channel(0);
            let mut scratch = data.to_vec();
            let median = median_f32_mut(&mut scratch);
            let mad = mad_f32_with_scratch(data, median, &mut scratch);
            let mut channels = ArrayVec::new();
            channels.push(MedianMad { median, mad });
            return Self {
                channels,
                quantization_sigma,
            };
        }

        let channels = (0..dimensions.channels())
            .into_par_iter()
            .map(|channel| {
                let data = image.channel(channel);
                let mut scratch = data.to_vec();
                let median = median_f32_mut(&mut scratch);
                let mad = mad_f32_with_scratch(data, median, &mut scratch);
                MedianMad { median, mad }
            })
            .collect::<Vec<_>>()
            .into_iter()
            .collect();
        Self {
            channels,
            quantization_sigma,
        }
    }
}

/// Image operations needed by the shared frame store.
pub(crate) trait StackableImage: Send + Sync + std::fmt::Debug + Sized {
    fn dimensions(&self) -> ImageDimensions;
    fn channel(&self, channel: usize) -> &[f32];
    fn metadata(&self) -> &ImageMetadata;
    fn load(path: &Path, context: &LoadContext) -> Result<Self, ImageError>;

    fn quantization_sigma(&self) -> Option<f32> {
        None
    }

    fn peek_dimensions(_path: &Path, _context: &LoadContext) -> Option<ImageDimensions> {
        None
    }

    fn into_planes(self) -> ArrayVec<Buffer2<f32>, 3>;
}

/// One planar f32 buffer, either resident or memory-mapped.
#[derive(Debug)]
pub(crate) enum StoredPlane {
    Memory(Buffer2<f32>),
    Mapped(Mmap),
}

impl StoredPlane {
    /// Memory-map a spilled plane file.
    pub(crate) fn map(path: PathBuf) -> Result<Self, FrameStoreError> {
        let file = File::open(&path).map_err(|source| FrameStoreError::OpenFile {
            path: path.clone(),
            source,
        })?;
        let mmap = unsafe {
            Mmap::map(&file).map_err(|source| FrameStoreError::MemoryMap {
                path: path.clone(),
                source,
            })?
        };
        #[cfg(unix)]
        {
            use memmap2::Advice;
            let _ = mmap.advise(Advice::Sequential);
        }
        Ok(Self::Mapped(mmap))
    }

    /// Samples the plane holds. The only geometry a stored plane knows — width and height are
    /// the cache's, not the plane's.
    #[inline]
    pub(crate) fn samples(&self) -> usize {
        match self {
            Self::Memory(buffer) => buffer.pixels().len(),
            Self::Mapped(mmap) => mmap.len() / size_of::<f32>(),
        }
    }

    #[inline]
    pub(crate) fn chunk(&self, start: usize, end: usize) -> &[f32] {
        match self {
            Self::Memory(buffer) => &buffer[start..end],
            Self::Mapped(mmap) => {
                bytemuck::cast_slice(&mmap[start * size_of::<f32>()..end * size_of::<f32>()])
            }
        }
    }
}

/// Which of a frame's planes a validation failure is about.
///
/// Names the plane in the errors below, and picks the range each one must satisfy: coverage is a
/// fraction of a pixel that had support, confidence an interpolation weight with no upper bound.
/// Carrying the kind rather than its label is what keeps that rule out of a string comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePlane {
    /// One of the image's colour planes.
    Channel,
    /// Per-pixel warp support, in `[0, 1]`.
    Coverage,
    /// Per-pixel interpolation confidence, non-negative.
    Confidence,
}

impl FramePlane {
    /// Whether `value` is in range for this plane. Non-finite is out of range for all of them.
    pub(crate) fn accepts(self, value: f32) -> bool {
        value.is_finite()
            && match self {
                // Finiteness is the whole rule for image data, as in `validate_sample_channels`:
                // dark subtraction takes a calibrated channel below zero legitimately.
                Self::Channel => true,
                Self::Coverage => (0.0..=1.0).contains(&value),
                Self::Confidence => value >= 0.0,
            }
    }
}

impl std::fmt::Display for FramePlane {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Channel => "a channel",
            Self::Coverage => "coverage",
            Self::Confidence => "confidence",
        })
    }
}

/// The per-pixel warp quality a registered light carries: how much of each output pixel had
/// support, and how confident the interpolation was.
///
/// Both are absent for a calibration frame and for a light loaded straight from disk, and they
/// travel together everywhere a frame does — so the two planes are converted, written and named
/// in one step rather than one apiece.
#[derive(Debug, Default)]
pub(crate) struct WarpQuality<P> {
    pub(crate) coverage: Option<P>,
    pub(crate) confidence: Option<P>,
}

impl<P> WarpQuality<P> {
    pub(crate) fn new(coverage: Option<P>, confidence: Option<P>) -> Self {
        Self {
            coverage,
            confidence,
        }
    }

    /// No warp quality at all — a calibration frame, or a light read straight from disk.
    pub(crate) fn none() -> Self {
        Self {
            coverage: None,
            confidence: None,
        }
    }

    fn map<Q>(self, mut convert: impl FnMut(P) -> Q) -> WarpQuality<Q> {
        WarpQuality {
            coverage: self.coverage.map(&mut convert),
            confidence: self.confidence.map(&mut convert),
        }
    }

    /// The plane `kind` names, when the frame carries it. [`FramePlane::Channel`] is not a warp
    /// quality plane and is always absent.
    pub(crate) fn plane(&self, kind: FramePlane) -> Option<&P> {
        match kind {
            FramePlane::Coverage => self.coverage.as_ref(),
            FramePlane::Confidence => self.confidence.as_ref(),
            FramePlane::Channel => None,
        }
    }

    /// Every plane the frame actually carries, each with the kind that names it. The one place
    /// that decides what "all the quality planes" means, so a caller cannot enumerate a subset.
    pub(crate) fn present(&self) -> impl Iterator<Item = (FramePlane, &P)> {
        [FramePlane::Coverage, FramePlane::Confidence]
            .into_iter()
            .filter_map(|kind| self.plane(kind).map(|plane| (kind, plane)))
    }

    /// How many planes are present, 0 to 2.
    pub(crate) fn count(&self) -> usize {
        self.present().count()
    }

    /// Whether the frame carries no warp quality at all.
    pub(crate) fn is_none(&self) -> bool {
        self.coverage.is_none() && self.confidence.is_none()
    }

    /// Convert each present plane, tagging it with the name its spill file carries. Which plane
    /// answers to which name is stated here alone, so a writer and a later reader cannot disagree.
    fn try_map<Q, E>(
        self,
        mut convert: impl FnMut(&'static str, P) -> Result<Q, E>,
    ) -> Result<WarpQuality<Q>, E> {
        Ok(WarpQuality {
            coverage: self.coverage.map(|p| convert("coverage", p)).transpose()?,
            confidence: self
                .confidence
                .map(|p| convert("confidence", p))
                .transpose()?,
        })
    }
}

/// One frame as the combine engine sees it: its channel planes, the per-pixel warp quality a
/// registered light carries (absent for calibration frames and for lights loaded straight from
/// disk), and the statistics measured on the source before any interpolation.
#[derive(Debug)]
pub(crate) struct StoredFrame {
    pub(crate) channels: ArrayVec<StoredPlane, 3>,
    pub(crate) quality: WarpQuality<StoredPlane>,
    pub(crate) source_stats: FrameStats,
}

impl StoredFrame {
    pub(crate) fn from_memory(
        image: impl StackableImage,
        quality: WarpQuality<Buffer2<f32>>,
        source_stats: FrameStats,
    ) -> Self {
        let channels = image
            .into_planes()
            .into_iter()
            .map(StoredPlane::Memory)
            .collect();
        Self {
            channels,
            quality: quality.map(StoredPlane::Memory),
            source_stats,
        }
    }

    /// Write the frame's channels and quality planes under `directory` and memory-map them back.
    pub(crate) fn spill(
        directory: &Path,
        name: &str,
        image: &impl StackableImage,
        quality: WarpQuality<Buffer2<f32>>,
        source_stats: FrameStats,
    ) -> Result<Self, FrameStoreError> {
        let spill = FrameSpill::new(directory, name);
        let channels = spill_channels(spill, image)?.planes;
        let quality = quality.try_map(|kind, plane| {
            let path = spill.quality_path(kind);
            write_plane(&path, &plane)?;
            StoredPlane::map(path)
        })?;
        Ok(Self {
            channels,
            quality,
            source_stats,
        })
    }
}

#[derive(Debug)]
struct SpillFiles {
    paths: ArrayVec<PathBuf, 3>,
}

#[derive(Debug)]
struct SpilledChannels {
    planes: ArrayVec<StoredPlane, 3>,
    paths: ArrayVec<PathBuf, 3>,
}

impl Drop for SpillFiles {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// A calibrated image stored on disk between detection and registration.
#[derive(Debug)]
pub(crate) struct StoredImage {
    pub(super) metadata: ImageMetadata,
    pub(super) dimensions: ImageDimensions,
    channels: ArrayVec<StoredPlane, 3>,
    _spill_files: SpillFiles,
}

impl StoredImage {
    /// Write `image`'s channels under `directory` and memory-map them back.
    pub(crate) fn spill(
        directory: &Path,
        name: &str,
        image: &LinearImage,
    ) -> Result<Self, FrameStoreError> {
        let dimensions = image.dimensions();
        let spilled = spill_channels(FrameSpill::new(directory, name), image)?;
        Ok(Self {
            metadata: image.metadata.clone(),
            dimensions,
            channels: spilled.planes,
            _spill_files: SpillFiles {
                paths: spilled.paths,
            },
        })
    }

    pub(crate) fn load(&self) -> LinearImage {
        let sample_count = self.dimensions.pixel_count();
        let planes = self
            .channels
            .iter()
            .map(|plane| plane.chunk(0, sample_count).to_vec());
        let mut image = LinearImage::from_planar_channels(self.dimensions, planes);
        image.metadata = self.metadata.clone();
        image
    }
}

fn spill_channels(
    spill: FrameSpill<'_>,
    image: &impl StackableImage,
) -> Result<SpilledChannels, FrameStoreError> {
    let dimensions = image.dimensions();
    let mut planes = ArrayVec::new();
    let mut paths = ArrayVec::new();
    for channel in 0..dimensions.channels() {
        let path = spill.channel_path(channel);
        write_plane(&path, image.channel(channel))?;
        planes.push(StoredPlane::map(path.clone())?);
        paths.push(path);
    }
    Ok(SpilledChannels { planes, paths })
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
    fn quality_path(self, kind: &str) -> PathBuf {
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
}

fn write_plane(path: &Path, pixels: &[f32]) -> Result<(), FrameStoreError> {
    let bytes = bytemuck::cast_slice(pixels);
    file_utils::publish_bytes(path, bytes, file_utils::PublicationMode::Cache).map_err(|source| {
        FrameStoreError::WriteFile {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(test)]
mod tests;
