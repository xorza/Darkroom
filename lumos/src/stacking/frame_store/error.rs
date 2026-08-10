//! Failures from the disk-backed frame store.

use std::path::PathBuf;

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
