//! One frame plane, wherever the memory tier put it.
//!
//! The whole of what makes a spilled run and a resident run the same code downstream: a plane is
//! either a `Buffer2` in RAM or a memory map over a file, and every read goes through the same
//! [`StoredPlane::chunk`] either way.

use std::fs::File;
use std::marker::PhantomData;
use std::path::Path;

use bytemuck::Pod;
use imaginarium::Buffer2;
use memmap2::Mmap;

use crate::frame_store::error::FrameStoreError;
use crate::frame_store::frame_spill;

/// One planar buffer of `T` — `f32` samples, or `u8` flags — either resident or memory-mapped.
#[derive(Debug)]
pub(crate) enum StoredPlane<T = f32> {
    Memory(Buffer2<T>),
    Mapped {
        map: Mmap,
        /// The handle of a run-scratch file, which Windows deletes when its last handle closes:
        /// held for the map's life, and dropped after it. Unix needs no handle, as the map keeps
        /// an unlinked file alive by itself.
        #[cfg(windows)]
        file: Option<File>,
        _type: PhantomData<T>,
    },
}

impl<T: Pod> StoredPlane<T> {
    /// Write `pixels` to `path` as the plane file [`Self::map`] reads back.
    pub(crate) fn write(path: &Path, pixels: &[T]) -> Result<(), FrameStoreError> {
        frame_spill::write_file(path, bytemuck::cast_slice(pixels))
    }

    /// Memory-map a spilled plane file.
    pub(crate) fn map(path: &Path) -> Result<Self, FrameStoreError> {
        Ok(Self::mapped(frame_spill::map_file(path)?, None))
    }

    /// Memory-map `file`, already open and written; `origin` names where it was for an error.
    pub(crate) fn map_open(file: File, origin: &Path) -> Result<Self, FrameStoreError> {
        // SAFETY: a run-scratch file has no name, so nothing but this handle can change it.
        let map = unsafe { Mmap::map(&file) }.map_err(|source| FrameStoreError::MemoryMap {
            path: origin.to_path_buf(),
            source,
        })?;
        Ok(Self::mapped(map, Some(file)))
    }

    fn mapped(map: Mmap, file: Option<File>) -> Self {
        #[cfg(unix)]
        {
            use memmap2::Advice;
            let _ = map.advise(Advice::Sequential);
            drop(file);
        }
        Self::Mapped {
            map,
            #[cfg(windows)]
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
            Self::Mapped { map, .. } => map.len() / size_of::<T>(),
        }
    }

    #[inline]
    pub(crate) fn chunk(&self, start: usize, end: usize) -> &[T] {
        match self {
            Self::Memory(buffer) => &buffer[start..end],
            Self::Mapped { map, .. } => {
                bytemuck::cast_slice(&map[start * size_of::<T>()..end * size_of::<T>()])
            }
        }
    }
}
