//! [`ExtensionChunks`]: how an extension is read, a chunk of rows of every plane at a time.

use std::ops::Range;

use arrayvec::ArrayVec;

use crate::io::fits::decode::plan;

/// The chunks an extension — flags, a flat gain, a mask or a quality plane — is read in: a chunk
/// of rows of every plane of its third axis, or of the one plane of an image of two, of at most a
/// chunk's bytes of its samples, a row at least. A section is plane-major, so each plane's samples
/// are one run of a chunk, and a tile that spans planes decodes once.
#[derive(Debug)]
pub(super) struct ExtensionChunks {
    shape: ArrayVec<usize, 3>,
    sample_bytes: usize,
    rows_per_chunk: usize,
}

impl ExtensionChunks {
    /// The chunks of an extension of `shape`, its samples `sample_bytes` wide, of at most
    /// `chunk_bytes` of them.
    pub(super) fn of(shape: &[usize], sample_bytes: usize, chunk_bytes: usize) -> Self {
        let shape: ArrayVec<usize, 3> =
            ArrayVec::try_from(shape).expect("the extension's check admitted at most three axes");
        debug_assert!(shape.len() >= 2, "an extension is a plane at least");
        let mut chunks = Self {
            shape,
            sample_bytes,
            rows_per_chunk: 1,
        };
        let row_bytes = chunks.shape[0] * chunks.planes() * sample_bytes;
        chunks.rows_per_chunk = (chunk_bytes / row_bytes.max(1)).clamp(1, chunks.shape[1].max(1));
        chunks
    }

    /// The planes the extension holds.
    pub(super) fn planes(&self) -> usize {
        self.shape.get(2).copied().unwrap_or(1)
    }

    /// The samples of one plane.
    pub(super) fn plane_len(&self) -> usize {
        self.shape[0] * self.shape[1]
    }

    /// The regions the read views in turn.
    pub(super) fn regions(&self) -> impl Iterator<Item = ArrayVec<Range<usize>, 3>> + '_ {
        plan::chunk_regions(&self.shape, self.rows_per_chunk)
    }

    /// What reading the extension holds: a chunk's words and the planes it reads into. What
    /// fits-well's reader holds for it is left out: a section's shape for a plain extension,
    /// and for a compressed one, which lumos never writes, its tiles.
    pub(super) fn read_bytes(&self) -> u64 {
        let words = (self.shape[0] * self.rows_per_chunk * self.planes() * self.sample_bytes)
            .next_multiple_of(size_of::<u64>());
        (words + self.plane_len() * self.planes() * self.sample_bytes) as u64
    }
}
