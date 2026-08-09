//! Which ancillary planes a combine is asked to produce.

/// Which ancillary per-pixel planes a combine should produce.
///
/// Each one is a full image-sized allocation — per channel for weight and variance — that the
/// combine writes whether or not anything reads it. A 60 MP RGB stack pays roughly 240 MB per
/// plane per channel, so a caller that only wants the combined image (a calibration master, a
/// quick preview) says so rather than paying for planes it discards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualityPlanes {
    /// Per-pixel coverage.
    pub coverage: bool,
    /// Per-channel sum of surviving frame weights.
    pub weight: bool,
    /// Per-channel linear-combine variance factor. A median has none whatever this says — it is
    /// not a linear combination — so requesting it is an upper bound, not a guarantee.
    pub variance: bool,
}

impl QualityPlanes {
    /// Every ancillary plane: the science default, and what makes the stacked master measurable.
    pub const ALL: Self = Self {
        coverage: true,
        weight: true,
        variance: true,
    };

    /// The combined image alone.
    pub const IMAGE_ONLY: Self = Self {
        coverage: false,
        weight: false,
        variance: false,
    };

    /// Drop the planes this combine method cannot produce, so the request reaching the reducer
    /// is exactly what it will write.
    pub(crate) fn resolve(self, produces_variance: bool) -> Self {
        Self {
            variance: self.variance && produces_variance,
            ..self
        }
    }
}

impl Default for QualityPlanes {
    fn default() -> Self {
        Self::ALL
    }
}
