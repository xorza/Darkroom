use imaginarium::Buffer2;

use crate::io::image::linear::LinearImage;
use crate::io::image::linear_pixels::LinearPixels;

/// A quality map that is either common to every image channel or channel-specific.
#[derive(Debug)]
pub enum QualityMap {
    /// One plane applies to every image channel.
    Shared(Buffer2<f32>),
    /// Each RGB image channel has its own plane.
    PerChannel([Buffer2<f32>; 3]),
}

impl QualityMap {
    /// Resolve the quality plane applicable to an image channel.
    pub fn channel(&self, channel: usize) -> &Buffer2<f32> {
        match self {
            Self::Shared(plane) => plane,
            Self::PerChannel(planes) => &planes[channel],
        }
    }

    pub(crate) fn from_pixels(pixels: LinearPixels) -> Self {
        match pixels {
            LinearPixels::L(plane) => Self::Shared(plane),
            LinearPixels::Rgb(planes) => Self::PerChannel(planes),
        }
    }
}

impl From<QualityMap> for LinearImage {
    fn from(map: QualityMap) -> Self {
        match map {
            QualityMap::Shared(plane) => plane.into(),
            QualityMap::PerChannel(planes) => planes.into(),
        }
    }
}

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

/// A stacked science product shared by statistical combine and drizzle.
///
/// Each plane is `Some` only when it was requested (see [`QualityPlanes`]) and the combine could
/// produce it. Each producer documents how it normalizes `coverage`: statistical combine reports
/// the fraction of frames with geometric support at a pixel, while drizzle reports accumulated
/// coverage relative to its maximum. Statistical quality is channel-specific because rejection
/// can retain different samples in each RGB channel; monochrome and drizzle quality use shared
/// planes.
#[derive(Debug)]
pub struct StackProduct {
    /// The combined linear image.
    pub image: LinearImage,
    /// Normalized per-pixel coverage in `[0, 1]`, for masking and fill gating.
    pub coverage: Option<Buffer2<f32>>,
    /// WHT map. Statistical combines store per-channel sums of surviving frame weights multiplied
    /// by per-pixel confidence; Equal becomes survivor count at unit confidence, while
    /// Noise/Manual normalize frame weights before that multiplier. Drizzle stores one shared
    /// plane of summed geometric drop weights.
    pub weight: Option<QualityMap>,
    /// Conditional linear-combine variance factor `Σwᵢ² / (Σwᵢ)²`.
    ///
    /// Present for weighted means and drizzle, using their actual surviving/contributing samples.
    /// Absent for median output because a median is not a linear combination.
    pub linear_variance: Option<QualityMap>,
    /// Source-quantization uncertainty carried through the combine, in the stacked image's
    /// sample units.
    ///
    /// Present when every input frame declared one and the frame set carries no coverage, which
    /// is what lets a surviving sample be traced back to the frame whose sigma and normalization
    /// gain it inherited. `None` otherwise.
    pub quantization_sigma: Option<f32>,
}
