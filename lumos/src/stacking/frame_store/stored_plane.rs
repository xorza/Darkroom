//! One frame plane, wherever the memory tier put it.
//!
//! The whole of what makes a spilled run and a resident run the same code downstream: a plane is
//! either a `Buffer2` in RAM or a memory map over a file, and every read goes through the same
//! [`StoredPlane::chunk`] either way.

use std::path::{Path, PathBuf};

use imaginarium::Buffer2;
use memmap2::Mmap;

use crate::stacking::frame_store::error::FrameStoreError;
use crate::stacking::frame_store::frame_spill;

/// One planar f32 buffer, either resident or memory-mapped.
#[derive(Debug)]
pub(crate) enum StoredPlane {
    Memory(Buffer2<f32>),
    Mapped(Mmap),
}

impl StoredPlane {
    /// Write `pixels` to `path` as the plane file [`Self::map`] reads back.
    pub(crate) fn write(path: &Path, pixels: &[f32]) -> Result<(), FrameStoreError> {
        frame_spill::write_file(path, bytemuck::cast_slice(pixels))
    }

    /// Memory-map a spilled plane file.
    pub(crate) fn map(path: PathBuf) -> Result<Self, FrameStoreError> {
        let mmap = frame_spill::map_file(&path)?;
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
