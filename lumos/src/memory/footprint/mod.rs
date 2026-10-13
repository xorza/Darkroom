//! [`Footprint`]: what a step on a frame's way holds.

/// What a step on a frame's way holds: what it keeps of the frame once done, and the most it
/// holds on the way, what it keeps included. Steps chain: each starts from what the one before
/// kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Footprint {
    pub(crate) kept: usize,
    pub(crate) peak: usize,
}

impl Footprint {
    /// A frame of `bytes`, which nothing has worked on yet.
    pub(crate) const fn kept(bytes: usize) -> Self {
        Self {
            kept: bytes,
            peak: bytes,
        }
    }

    /// This, then a step that holds `bytes` beside what this keeps and leaves it as it was.
    pub(crate) const fn beside(self, bytes: usize) -> Self {
        let during = self.kept.saturating_add(bytes);
        Self {
            kept: self.kept,
            peak: if during > self.peak {
                during
            } else {
                self.peak
            },
        }
    }

    /// This, then `next`, a step that starts from what this keeps: its peak counts that too.
    pub(crate) const fn then(self, next: Self) -> Self {
        Self {
            kept: next.kept,
            peak: if next.peak > self.peak {
                next.peak
            } else {
                self.peak
            },
        }
    }

    /// What the way holds beyond what it keeps at its end.
    pub(crate) const fn transient(self) -> usize {
        self.peak.saturating_sub(self.kept)
    }
}

#[cfg(test)]
mod tests;
