use std::ops::Range;

use serde::{Deserialize, Serialize};

/// Compact `(start, len)` index range into a flat pool. 8 bytes,
/// `Copy`. Cheaper to store than `Range<usize>` (16 bytes) when many
/// spans live in cache rows or per-node SoA columns.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    pub start: u32,
    pub len: u32,
}

impl Span {
    pub const fn new(start: u32, len: u32) -> Self {
        Self { start, len }
    }

    /// `Range<usize>` form — slice with `&pool[span.range()]` or
    /// iterate via `for i in span.range()`.
    pub const fn range(self) -> Range<usize> {
        let start = self.start as usize;
        start..start + self.len as usize
    }
}
