//! One frame plane, wherever the memory tier put it.
//!
//! The whole of what makes a spilled run and a resident run the same code downstream: a plane is
//! either a `Buffer2` in RAM or a memory map over a file, and every read goes through the same
//! [`StoredPlane::chunk`] either way.

use std::marker::PhantomData;
use std::path::Path;

use bytemuck::Pod;
use common::file_utils;
use imaginarium::Buffer2;

use crate::frame_store::error::FrameStoreError;
use crate::platform::MappedFile;

/// One planar buffer of `T` — `f32` samples, or `u8` flags — either resident or memory-mapped.
#[derive(Debug)]
pub(crate) enum StoredPlane<T = f32> {
    Memory(Buffer2<T>),
    Mapped {
        file: MappedFile,
        _type: PhantomData<T>,
    },
}

impl<T: Pod> StoredPlane<T> {
    /// Write `pixels` to `path` as the plane file [`Self::map`] reads back, published whole: a
    /// reader finds the old file or the new one, never a part.
    pub(crate) fn write(path: &Path, pixels: &[T]) -> Result<(), FrameStoreError> {
        file_utils::publish_bytes(
            path,
            bytemuck::cast_slice(pixels),
            file_utils::PublicationMode::Cache,
        )
        .map_err(|source| FrameStoreError::WriteFile {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Memory-map a plane file [`Self::write`] wrote.
    pub(crate) fn map(path: &Path) -> Result<Self, FrameStoreError> {
        // SAFETY: lumos publishes every named file it maps whole and never rewrites one in place;
        // a file replaced by another process after the map is the documented mmap hazard.
        let file =
            unsafe { MappedFile::open(path) }.map_err(|source| FrameStoreError::MemoryMap {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(Self::mapped(file))
    }

    pub(crate) const fn mapped(file: MappedFile) -> Self {
        Self::Mapped {
            file,
            _type: PhantomData,
        }
    }

    /// Samples the plane holds. The only geometry a stored plane knows — width and height are
    /// the cache's, not the plane's.
    #[inline]
    pub(crate) fn samples(&self) -> usize {
        match self {
            Self::Memory(buffer) => buffer.pixels().len(),
            Self::Mapped { file, .. } => file.bytes().len() / size_of::<T>(),
        }
    }

    /// Every sample, as bytes.
    pub(crate) fn bytes(&self) -> &[u8] {
        match self {
            Self::Memory(buffer) => bytemuck::cast_slice(buffer.pixels()),
            Self::Mapped { file, .. } => file.bytes(),
        }
    }

    #[inline]
    pub(crate) fn chunk(&self, start: usize, end: usize) -> &[T] {
        match self {
            Self::Memory(buffer) => &buffer[start..end],
            Self::Mapped { file, .. } => {
                bytemuck::cast_slice(&file.bytes()[start * size_of::<T>()..end * size_of::<T>()])
            }
        }
    }
}
