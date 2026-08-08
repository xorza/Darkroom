use imaginarium::Buffer2;

use crate::io::image::cfa::CfaImage;
use crate::io::image::linear::LinearImage;
use crate::io::image::linear_pixels::LinearPixels;
use crate::math::size2us::Size2us;

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
    /// Normalized coverage in `[0, 1]`, for masking and fill gating.
    pub coverage: Option<Coverage>,
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

/// Normalized coverage in `[0, 1]`: the share of frames that reached each pixel.
///
/// A sum type because the usual answer is a single number. When no frame carries a coverage map
/// every pixel saw every frame, and spelling that out as a plane of `1.0` costs a full image-sized
/// allocation — 240 MB at 60 MP — to carry one constant. Drizzle and warped stacks, where frames
/// genuinely cover different regions, produce [`Coverage::PerPixel`].
#[derive(Debug, Clone)]
pub enum Coverage {
    /// Every pixel covered to the same degree.
    Uniform {
        value: f32,
        /// Kept so the plane can still be materialized on demand.
        size: Size2us,
    },
    /// Coverage measured per pixel.
    PerPixel(Buffer2<f32>),
}

impl Coverage {
    /// Pixel extent of the coverage this describes.
    pub fn size(&self) -> Size2us {
        match self {
            Coverage::Uniform { size, .. } => *size,
            Coverage::PerPixel(plane) => Size2us::new(plane.width(), plane.height()),
        }
    }

    /// The measured plane, or `None` when coverage is uniform and no plane exists.
    pub fn per_pixel(&self) -> Option<&Buffer2<f32>> {
        match self {
            Coverage::Uniform { .. } => None,
            Coverage::PerPixel(plane) => Some(plane),
        }
    }

    /// Materialize as a plane. Allocates for [`Coverage::Uniform`] — the cost this type exists to
    /// let a caller avoid, so only reach for it when a plane is genuinely what is wanted.
    pub fn to_plane(&self) -> Buffer2<f32> {
        match self {
            Coverage::Uniform { value, size } => {
                Buffer2::new_filled(size.width, size.height, *value)
            }
            Coverage::PerPixel(plane) => plane.clone(),
        }
    }
}

/// Indexes like the plane it stands for, by flat sample or by `(x, y)` — a uniform coverage
/// answers with its constant rather than materializing anything.
impl std::ops::Index<usize> for Coverage {
    type Output = f32;

    fn index(&self, index: usize) -> &f32 {
        match self {
            Coverage::Uniform { value, size } => {
                debug_assert!(index < size.pixel_count(), "coverage index out of range");
                value
            }
            Coverage::PerPixel(plane) => &plane[index],
        }
    }
}

impl std::ops::Index<(usize, usize)> for Coverage {
    type Output = f32;

    fn index(&self, (x, y): (usize, usize)) -> &f32 {
        match self {
            Coverage::Uniform { value, size } => {
                debug_assert!(
                    x < size.width && y < size.height,
                    "coverage index out of range"
                );
                value
            }
            Coverage::PerPixel(plane) => &plane[(x, y)],
        }
    }
}

impl From<Coverage> for LinearImage {
    fn from(coverage: Coverage) -> Self {
        match coverage {
            Coverage::PerPixel(plane) => plane.into(),
            uniform => uniform.to_plane().into(),
        }
    }
}

impl StackProduct {
    /// Reinterpret a combined mosaic stack as the calibration master it is.
    ///
    /// # Panics
    ///
    /// If the product has more than one channel. A CFA frame is a single mosaic plane, so a
    /// stack of them is too — `CfaImage` has nowhere to put a second channel and no loader
    /// produces one.
    pub(crate) fn into_cfa_master(self) -> CfaImage {
        assert_eq!(
            self.image.channels(),
            1,
            "a CFA master must be single-channel; got {} channels",
            self.image.channels()
        );
        CfaImage {
            data: self.image.pixels.into_l(),
            metadata: self.image.metadata,
            quantization_sigma: self.quantization_sigma,
        }
    }
}
