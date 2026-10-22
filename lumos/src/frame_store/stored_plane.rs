//! One frame plane, wherever the memory tier put it.
//!
//! The whole of what makes a spilled run and a resident run the same code downstream: a plane is
//! either a `Buffer2` in RAM or a memory map over a file, and every read goes through the same
//! [`StoredPlane::chunk`] either way.

use std::fs::File;
use std::io::{self, Write};
use std::marker::PhantomData;
use std::ops::Range;
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
    /// Write `pixels` to `path` as the plane file [`Self::map`] reads back, published whole — a
    /// reader finds the old file or the new one, never a part — and mapped from the handle that
    /// wrote it: another run that publishes the same name after it is never read in its place.
    pub(crate) fn publish(path: &Path, pixels: &[T]) -> Result<Self, FrameStoreError> {
        let mut mapped = None;
        file_utils::publish(
            path,
            file_utils::PublicationMode::Cache,
            |file: &mut File| -> io::Result<()> {
                file.write_all(bytemuck::cast_slice(pixels))?;
                // A published file is replaced only by a rename, never written in place.
                mapped = Some(unsafe { MappedFile::of_file(file) }?);
                Ok(())
            },
        )
        .map_err(|source| FrameStoreError::WriteFile {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Self::mapped(
            mapped.expect("a published plane was mapped as it was written"),
        ))
    }

    /// Memory-map a plane file [`Self::publish`] wrote.
    pub(crate) fn map(path: &Path) -> Result<Self, FrameStoreError> {
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

    /// The samples `range` covers.
    #[inline]
    pub(crate) fn chunk(&self, range: Range<usize>) -> &[T] {
        match self {
            Self::Memory(buffer) => &buffer[range],
            Self::Mapped { file, .. } => bytemuck::cast_slice(
                &file.bytes()[range.start * size_of::<T>()..range.end * size_of::<T>()],
            ),
        }
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use bytemuck::Pod;

    use crate::frame_store::stored_plane::StoredPlane;

    impl<T: Pod> StoredPlane<T> {
        /// Drop a mapped plane's pages from the page cache, so its next read comes from the disk;
        /// `false` for a resident plane, or when a page stays: see [`MappedFile::evict`].
        ///
        /// [`MappedFile::evict`]: crate::platform::MappedFile::evict
        pub(crate) fn evict(&self) -> bool {
            match self {
                Self::Memory(_) => false,
                Self::Mapped { file, .. } => file.evict(),
            }
        }

        /// The share of a mapped plane's pages in memory; `None` for a resident plane, or where
        /// the system does not say.
        #[cfg(test)]
        pub(crate) fn resident_share(&self) -> Option<f64> {
            match self {
                Self::Memory(_) => None,
                Self::Mapped { file, .. } => file.resident_share(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use common::TempDir;

    use crate::frame_store::run_scratch::RunScratch;

    /// A scratch plane of 4 MiB is in memory once written, out of it once evicted, and back in once
    /// read through: every page, by `mincore`, each time, where the system both evicts and says
    /// what is resident — which it does both or neither of — and it reads the same either way.
    #[test]
    fn an_evicted_plane_reads_from_the_disk() {
        let dir = TempDir::new("stored_plane_evict");
        let scratch = RunScratch::create_evictable(dir.path()).unwrap();
        let samples = 1 << 20;
        let plane = scratch.store(&vec![0.5f32; samples]).unwrap();
        let written = plane.resident_share();
        let evicted = plane.evict();
        assert_eq!(evicted, written.is_some());
        if evicted {
            assert_eq!(written, Some(1.0));
            assert_eq!(plane.resident_share(), Some(0.0));
        }
        let sum: f64 = plane.chunk(0..samples).iter().map(|&v| f64::from(v)).sum();
        assert_eq!(sum, 0.5 * samples as f64);
        assert_eq!(plane.resident_share(), written);
    }
}
