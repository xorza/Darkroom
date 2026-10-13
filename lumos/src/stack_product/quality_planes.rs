//! Which ancillary planes a combine is asked to produce.

use crate::combine::stack_config::CombineMethod;
use crate::image::image_dimensions::ImageDimensions;

/// Which ancillary per-pixel planes a combine should produce.
///
/// Each one is a full image-sized allocation — per channel for all but coverage — that the
/// combine writes whether or not anything reads it. A 60 MP RGB stack pays roughly 240 MB per
/// plane per channel, so a caller that only wants the combined image (a calibration master, a
/// quick preview) says so rather than paying for planes it discards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualityPlanes {
    /// Per-pixel coverage.
    pub coverage: bool,
    /// Per-channel sum of surviving frame weights.
    pub weight: bool,
    /// Per-channel inverse variance of the linear combine. A median has none whatever this says —
    /// it is not a linear combination — so requesting it is an upper bound, not a guarantee.
    pub inverse_variance: bool,
    /// Per-channel scatter of the surviving samples, the variance of their weighted mean as the
    /// frames show it. A statistical mean combine produces it; a median or a drizzle does not.
    pub dispersion: bool,
}

impl QualityPlanes {
    /// Coverage, weight and inverse variance: the science default, and what makes the stacked
    /// master measurable.
    pub const STANDARD: Self = Self {
        coverage: true,
        weight: true,
        inverse_variance: true,
        dispersion: false,
    };

    /// Every ancillary plane: the standard ones, and the dispersion that checks the inverse
    /// variance.
    pub const ALL: Self = Self {
        dispersion: true,
        ..Self::STANDARD
    };

    /// The combined image alone.
    pub const IMAGE_ONLY: Self = Self {
        coverage: false,
        weight: false,
        inverse_variance: false,
        dispersion: false,
    };

    /// Image-sized planes a combine keeps resident per output channel: the combined pixels, plus
    /// whichever of weight, inverse variance and dispersion were asked for and so are allocated up
    /// front.
    const fn resident_planes_per_channel(self) -> usize {
        1 + self.weight as usize + self.inverse_variance as usize + self.dispersion as usize
    }

    /// The bytes a combine's output holds a pixel: these planes for each of `channels`, the
    /// combined pixels among them, `coverage_planes` coverage planes, and a flag byte when `flags`.
    pub(crate) const fn pixel_bytes(
        self,
        channels: usize,
        coverage_planes: usize,
        flags: bool,
    ) -> usize {
        (channels * self.resident_planes_per_channel() + coverage_planes) * size_of::<f32>()
            + flags as usize
    }

    /// The bytes a combine's output of `dimensions` holds before its frames are seen: one coverage
    /// plane if asked for, and the flag plane. An upper bound for a run whose frames turn out to
    /// carry no quality planes, whose coverage is a constant, or no flags, which leave the flag
    /// plane out.
    pub(crate) const fn heap_bytes(self, dimensions: ImageDimensions) -> usize {
        self.pixel_bytes(dimensions.channels(), self.coverage as usize, true)
            * dimensions.pixel_count()
    }

    /// Drop the planes `method` cannot produce, so the request reaching the reducer is exactly
    /// what it will write: inverse variance and dispersion belong to a weighted mean.
    pub(crate) const fn resolve(self, method: CombineMethod) -> Self {
        let weighted_mean = matches!(method, CombineMethod::Mean(_));
        Self {
            inverse_variance: self.inverse_variance && weighted_mean,
            dispersion: self.dispersion && weighted_mean,
            ..self
        }
    }
}

impl Default for QualityPlanes {
    fn default() -> Self {
        Self::STANDARD
    }
}
