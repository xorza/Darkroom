//! [`ChunkMemoryLayout`]: what one combine pass holds, and the rows that leaves room for.

use crate::math::size2us::Size2us;
use crate::memory;

pub(super) const MIN_CHUNK_ROWS: usize = 64;

/// What one combine pass holds in memory, so [`ChunkMemoryLayout::chunk_rows`] can price a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChunkMemoryLayout {
    /// Bytes read per pixel of the active row chunk, across every plane read concurrently: four
    /// for each f32 plane, one for a flag plane.
    pub(crate) input_bytes: usize,
    /// Bytes per pixel of the image-sized planes held throughout chunk processing: four for each
    /// f32 plane, one for the flag plane.
    pub(crate) resident_bytes: usize,
}

/// The rows a combine pass reads at once, and what that holds beyond the budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChunkRows {
    pub(crate) rows: usize,
    /// Bytes past the budget: nonzero only when not even [`MIN_CHUNK_ROWS`] fit beside the
    /// resident planes, and the pass holds that floor anyway so it still makes progress.
    pub(crate) overcommit_bytes: u64,
}

impl ChunkMemoryLayout {
    /// Rows a combine may hold at once: the budget left after the resident planes, divided by the
    /// cost of a row, floored at [`MIN_CHUNK_ROWS`] so a tight budget still makes progress.
    pub(crate) fn chunk_rows(self, size: Size2us, available_memory: u64) -> ChunkRows {
        let bytes_per_row = (size.width as u64).saturating_mul(self.input_bytes as u64);
        let resident_bytes = (size.width as u64)
            .saturating_mul(size.height as u64)
            .saturating_mul(self.resident_bytes as u64);
        let budget = memory::memory_budget(available_memory);
        if bytes_per_row == 0 {
            return ChunkRows {
                rows: MIN_CHUNK_ROWS,
                overcommit_bytes: 0,
            };
        }
        let fit = budget.saturating_sub(resident_bytes) / bytes_per_row;
        if fit >= MIN_CHUNK_ROWS as u64 {
            return ChunkRows {
                rows: fit.min(usize::MAX as u64) as usize,
                overcommit_bytes: 0,
            };
        }
        let floor_bytes = bytes_per_row.saturating_mul(MIN_CHUNK_ROWS.min(size.height) as u64);
        ChunkRows {
            rows: MIN_CHUNK_ROWS,
            overcommit_bytes: resident_bytes
                .saturating_add(floor_bytes)
                .saturating_sub(budget),
        }
    }
}
