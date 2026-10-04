//! [`ChunkMemoryLayout`]: what one combine pass holds, and the rows that leaves room for.

use crate::math::size2us::Size2us;
use crate::memory;

pub(super) const MIN_CHUNK_ROWS: usize = 64;

/// What one combine pass holds in memory, so [`ChunkMemoryLayout::optimal_chunk_rows`] can price a
/// row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChunkMemoryLayout {
    /// Bytes read per pixel of the active row chunk, across every plane read concurrently: four
    /// for each f32 plane, one for a flag plane.
    pub(crate) input_bytes: usize,
    /// Full image-sized planes held throughout chunk processing.
    pub(crate) resident_planes: usize,
}

impl ChunkMemoryLayout {
    /// Rows a combine may hold at once: the budget left after the resident planes, divided by the
    /// cost of a row, floored at [`MIN_CHUNK_ROWS`] so a tight budget still makes progress.
    pub(crate) fn optimal_chunk_rows(self, size: Size2us, available_memory: u64) -> usize {
        let bytes_per_row = size
            .width
            .checked_mul(self.input_bytes)
            .map_or(u64::MAX, |value| value as u64);
        if bytes_per_row == 0 {
            return MIN_CHUNK_ROWS;
        }
        let resident_bytes = size
            .width
            .checked_mul(size.height)
            .and_then(|value| value.checked_mul(self.resident_planes))
            .and_then(|value| value.checked_mul(size_of::<f32>()))
            .map_or(u64::MAX, |value| value as u64);
        (memory::memory_budget(available_memory).saturating_sub(resident_bytes) / bytes_per_row)
            .max(MIN_CHUNK_ROWS as u64) as usize
    }
}
