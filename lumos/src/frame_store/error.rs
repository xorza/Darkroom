//! Failures from the disk-backed frame store.

use std::io;
use std::path::PathBuf;

use crate::frame::capture_condition::CaptureCondition;
use crate::io::error::ImageError;

/// Failure while creating or accessing disk-backed frame storage.
#[derive(Debug, thiserror::Error)]
pub enum FrameStoreError {
    #[error("failed to create frame-store directory '{path}': {source}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to write frame-store file '{path}': {source}")]
    WriteFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to open frame-store file '{path}': {source}")]
    OpenFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to read metadata for frame-store source '{path}': {source}")]
    ReadMetadata {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("frame-store source changed while it was being read: '{path}'")]
    SourceChanged { path: PathBuf },
    #[error(
        "frame-store directory '{path}' is on {filesystem}, which keeps its files in memory: a \
         spill there spends the RAM it was meant to free; set `cache_dir` to a directory on disk"
    )]
    MemoryBackedDirectory { path: PathBuf, filesystem: String },
    /// The disk a spill goes to holds less than the run will write there: refused before the
    /// first frame is decoded rather than after hours of them. A figure, not a reservation:
    /// another writer can still fill the disk later.
    #[error(
        "frame-store directory '{path}' has {available} bytes free, and the spill needs {needed}; \
         free space there or set `cache_dir` to a larger disk"
    )]
    InsufficientSpace {
        path: PathBuf,
        needed: u64,
        available: u64,
    },
    #[error("failed to memory-map frame-store file '{path}': {source}")]
    MemoryMap {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Frame `index` of a set was taken under another exposure or sensor temperature than an earlier
/// frame of it, where the set has to share one: a dark master's thermal signal is one exposure's
/// at one temperature.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error(
    "frame {index} has {condition} {value}, outside the tolerance of frame {reference_index}'s \
     {reference}"
)]
pub struct ConditionMismatch {
    pub condition: CaptureCondition,
    pub index: usize,
    pub value: f64,
    pub reference_index: usize,
    pub reference: f64,
}

impl ConditionMismatch {
    /// The error with each frame index it holds, a position in the list it was raised over, read
    /// through `inputs` into the caller's own numbering of those frames.
    pub(crate) const fn renumbered(self, inputs: &[usize]) -> Self {
        Self {
            index: inputs[self.index],
            reference_index: inputs[self.reference_index],
            ..self
        }
    }
}

/// Why a frame could not be read through the decode cache: its decode, or the cache.
#[derive(Debug, thiserror::Error)]
pub(crate) enum KeptDecodeError {
    #[error(transparent)]
    Decode(#[from] ImageError),
    #[error(transparent)]
    Store(#[from] FrameStoreError),
}
