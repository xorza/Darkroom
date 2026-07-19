//! Memory planning and RAM/mmap storage shared by stacking stages.

use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{BufWriter, Write};
use std::mem::size_of;
use std::path::{Path, PathBuf};

use arrayvec::ArrayVec;
use imaginarium::Buffer2;
use memmap2::Mmap;
use rayon::prelude::*;

use common::FnvHasher;

use crate::io::astro_image::error::ImageError;
use crate::io::astro_image::{AstroImage, AstroImageMetadata, ImageDimensions, PixelData};
use crate::math::statistics::{ChannelStats, mad_f32_with_scratch, median_f32_mut};

/// Failure while creating or accessing disk-backed frame storage.
#[derive(Debug, thiserror::Error)]
pub enum FrameStoreError {
    #[error("failed to create frame-store directory '{path}': {source}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to create frame-store file '{path}': {source}")]
    CreateFile {
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
#[derive(Debug, Clone)]
pub(crate) struct FrameStats {
    pub(crate) channels: ArrayVec<ChannelStats, 3>,
}

pub(crate) fn compute_frame_stats(image: &impl StackableImage) -> FrameStats {
    let dimensions = image.dimensions();
    if dimensions.channels() == 1 {
        let data = image.channel(0);
        let mut scratch = data.to_vec();
        let median = median_f32_mut(&mut scratch);
        let mad = mad_f32_with_scratch(data, median, &mut scratch);
        let mut channels = ArrayVec::new();
        channels.push(ChannelStats { median, mad });
        return FrameStats { channels };
    }

    let channels = (0..dimensions.channels())
        .into_par_iter()
        .map(|channel| {
            let data = image.channel(channel);
            let mut scratch = data.to_vec();
            let median = median_f32_mut(&mut scratch);
            let mad = mad_f32_with_scratch(data, median, &mut scratch);
            ChannelStats { median, mad }
        })
        .collect::<Vec<_>>()
        .into_iter()
        .collect();
    FrameStats { channels }
}

/// Image operations needed by the shared frame store.
pub(crate) trait StackableImage: Send + Sync + std::fmt::Debug + Sized {
    fn dimensions(&self) -> ImageDimensions;
    fn channel(&self, channel: usize) -> &[f32];
    fn metadata(&self) -> &AstroImageMetadata;
    fn load(path: &Path) -> Result<Self, ImageError>;

    fn peek_dimensions(_path: &Path) -> Option<ImageDimensions> {
        None
    }

    fn into_planes(self) -> ArrayVec<Buffer2<f32>, 3>;

    fn from_stacked(
        pixels: PixelData,
        metadata: AstroImageMetadata,
        dimensions: ImageDimensions,
    ) -> Self;
}

/// One planar f32 buffer, either resident or memory-mapped.
#[derive(Debug)]
pub(crate) enum StoredPlane {
    Memory(Buffer2<f32>),
    Mapped(Mmap),
}

impl StoredPlane {
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

/// Stored channels for an unweighted frame.
#[derive(Debug)]
pub(crate) struct StoredFrame {
    pub(crate) channels: ArrayVec<StoredPlane, 3>,
}

/// Stored channels, coverage, and statistics for one registered light frame.
#[derive(Debug)]
pub(crate) struct StoredLightFrame {
    pub(crate) channels: ArrayVec<StoredPlane, 3>,
    pub(crate) coverage: Option<StoredPlane>,
    pub(crate) stats: FrameStats,
}

impl StoredLightFrame {
    pub(crate) fn from_memory(image: AstroImage, coverage: Option<Buffer2<f32>>) -> Self {
        let stats = compute_frame_stats(&image);
        let channels = image
            .into_planes()
            .into_iter()
            .map(StoredPlane::Memory)
            .collect();
        Self {
            channels,
            coverage: coverage.map(StoredPlane::Memory),
            stats,
        }
    }

    pub(crate) fn from_stored(frame: StoredFrame, stats: FrameStats) -> Self {
        Self {
            channels: frame.channels,
            coverage: None,
            stats,
        }
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
    pub(crate) metadata: AstroImageMetadata,
    pub(crate) dimensions: ImageDimensions,
    channels: ArrayVec<StoredPlane, 3>,
    _spill_files: SpillFiles,
}

impl StoredImage {
    pub(crate) fn load(&self) -> AstroImage {
        let sample_count = self.dimensions.pixel_count();
        let planes = self
            .channels
            .iter()
            .map(|plane| plane.chunk(0, sample_count).to_vec());
        let mut image = AstroImage::from_planar_channels(self.dimensions, planes);
        image.metadata = self.metadata.clone();
        image
    }
}

pub(crate) fn store_image(
    directory: &Path,
    name: &str,
    image: &AstroImage,
) -> Result<StoredImage, FrameStoreError> {
    let dimensions = image.dimensions();
    let spilled = spill_channels(directory, name, image)?;
    Ok(StoredImage {
        metadata: image.metadata.clone(),
        dimensions,
        channels: spilled.planes,
        _spill_files: SpillFiles {
            paths: spilled.paths,
        },
    })
}

pub(crate) fn store_light_frame(
    directory: &Path,
    name: &str,
    image: AstroImage,
    coverage: Option<Buffer2<f32>>,
) -> Result<StoredLightFrame, FrameStoreError> {
    let stats = compute_frame_stats(&image);
    let channels = spill_channels(directory, name, &image)?.planes;
    let coverage = match coverage {
        Some(coverage) => {
            let path = directory.join(format!("{name}_coverage.bin"));
            write_plane(&path, &coverage)?;
            Some(StoredPlane::Mapped(map_plane(path)?))
        }
        None => None,
    };
    Ok(StoredLightFrame {
        channels,
        coverage,
        stats,
    })
}

pub(crate) fn store_frame(
    directory: &Path,
    name: &str,
    image: &impl StackableImage,
) -> Result<StoredFrame, FrameStoreError> {
    Ok(StoredFrame {
        channels: spill_channels(directory, name, image)?.planes,
    })
}

pub(crate) fn frame_from_memory<I: StackableImage>(image: I) -> StoredFrame {
    StoredFrame {
        channels: image
            .into_planes()
            .into_iter()
            .map(StoredPlane::Memory)
            .collect(),
    }
}

fn spill_channels(
    directory: &Path,
    name: &str,
    image: &impl StackableImage,
) -> Result<SpilledChannels, FrameStoreError> {
    let dimensions = image.dimensions();
    let mut planes = ArrayVec::new();
    let mut paths = ArrayVec::new();
    for channel in 0..dimensions.channels() {
        let path = directory.join(channel_filename(name, channel));
        write_plane(&path, image.channel(channel))?;
        planes.push(StoredPlane::Mapped(map_plane(path.clone())?));
        paths.push(path);
    }
    Ok(SpilledChannels { planes, paths })
}

pub(crate) fn frame_bytes(dimensions: ImageDimensions) -> usize {
    dimensions.sample_count() * size_of::<f32>()
}

/// Statistics hold a full-frame scratch buffer beside the decoded pixels.
const DECODE_TRANSIENT_FACTOR: usize = 2;

pub(crate) fn decode_transient_bytes(dimensions: ImageDimensions) -> usize {
    DECODE_TRANSIENT_FACTOR * frame_bytes(dimensions)
}

pub(crate) fn cache_filename(path: &Path) -> String {
    let mut hasher = FnvHasher::new();
    path.hash(&mut hasher);
    format!("{:016x}.bin", hasher.finish())
}

pub(crate) fn channel_filename(name: &str, channel: usize) -> String {
    let stem = name.strip_suffix(".bin").unwrap_or(name);
    format!("{stem}_c{channel}.bin")
}

pub(crate) fn reusable_plane(path: &Path, dimensions: ImageDimensions) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    let expected = (dimensions.pixel_count() * size_of::<f32>()) as u64;
    metadata.len() == expected
}

pub(crate) fn write_plane(path: &Path, pixels: &[f32]) -> Result<(), FrameStoreError> {
    let file = File::create(path).map_err(|source| FrameStoreError::CreateFile {
        path: path.to_path_buf(),
        source,
    })?;
    let mut writer = BufWriter::new(file);
    let bytes = bytemuck::cast_slice(pixels);
    writer
        .write_all(bytes)
        .and_then(|()| writer.flush())
        .map_err(|source| FrameStoreError::WriteFile {
            path: path.to_path_buf(),
            source,
        })
}

pub(crate) fn map_plane(path: PathBuf) -> Result<Mmap, FrameStoreError> {
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
    Ok(mmap)
}

pub(crate) const MIN_CHUNK_ROWS: usize = 64;
/// Leaves operating-system and measurement-fluctuation headroom.
const MEMORY_PERCENT: u64 = 75;

pub(crate) fn memory_budget(available_memory: u64) -> u64 {
    (available_memory as u128 * MEMORY_PERCENT as u128 / 100) as u64
}

pub(crate) fn optimal_chunk_rows(
    width: usize,
    channels: usize,
    frame_count: usize,
    available_memory: u64,
) -> usize {
    let bytes_per_row = width
        .checked_mul(channels)
        .and_then(|value| value.checked_mul(size_of::<f32>()))
        .and_then(|value| value.checked_mul(frame_count))
        .map(|value| value as u64)
        .unwrap_or(u64::MAX);
    if bytes_per_row == 0 {
        return MIN_CHUNK_ROWS;
    }
    (memory_budget(available_memory) / bytes_per_row).max(MIN_CHUNK_ROWS as u64) as usize
}

pub(crate) fn load_concurrency(
    resident_bytes_per_frame: usize,
    transient_bytes_per_decode: usize,
    resident_frames: usize,
    available_memory: u64,
    max_workers: usize,
) -> usize {
    let usable = memory_budget(available_memory);
    let transient = (transient_bytes_per_decode as u64).max(1);
    let resident = (resident_bytes_per_frame as u64).saturating_mul(resident_frames as u64);
    let headroom = usable.saturating_sub(resident);
    ((headroom / transient).max(1) as usize).min(max_workers.max(1))
}

pub(crate) fn fits_in_memory(
    bytes_per_image: usize,
    frame_count: usize,
    available_memory: u64,
) -> bool {
    bytes_per_image
        .checked_mul(frame_count)
        .is_some_and(|bytes| bytes as u64 <= memory_budget(available_memory))
}

/// Calibrated/warped pixels plus detection or warp scratch.
pub(crate) const PER_FRAME_WORKING_PLANES: usize = 8;
/// Adds the demosaic arena held during decode.
pub(crate) const PER_FRAME_DECODE_PLANES: usize = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MemoryPlan {
    pub(crate) fits_in_ram: bool,
    pub(crate) decode_concurrency: usize,
    pub(crate) warp_concurrency: usize,
}

pub(crate) fn plan_memory(
    plane_bytes: usize,
    frame_count: usize,
    threads: usize,
    available: u64,
) -> MemoryPlan {
    let warped_bytes = 4 * plane_bytes;
    let concurrent = frame_count.min(threads);
    let scratch_reserve = (PER_FRAME_WORKING_PLANES as u64)
        .saturating_mul(plane_bytes as u64)
        .saturating_mul(concurrent as u64);
    let frame_budget = available.saturating_sub(scratch_reserve);
    let fits_in_ram = fits_in_memory(warped_bytes, frame_count, frame_budget);
    let decode_concurrency = load_concurrency(
        plane_bytes,
        PER_FRAME_DECODE_PLANES * plane_bytes,
        0,
        available,
        threads,
    );
    let warp_concurrency = load_concurrency(
        plane_bytes,
        PER_FRAME_WORKING_PLANES * plane_bytes,
        0,
        available,
        threads,
    );
    MemoryPlan {
        fits_in_ram,
        decode_concurrency,
        warp_concurrency,
    }
}

#[cfg(test)]
mod tests;
