use imaginarium::Buffer2;

use crate::io::astro_image::{LinearImage, PixelData};

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

    pub(crate) fn from_pixels(pixels: PixelData) -> Self {
        match pixels {
            PixelData::L(image) => {
                let [plane] = image.channels;
                Self::Shared(plane)
            }
            PixelData::Rgb(image) => Self::PerChannel(image.channels),
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

/// A stacked science product shared by statistical combine and drizzle.
///
/// Each producer documents how it normalizes `coverage`: statistical combine reports the fraction
/// of frames with geometric support at a pixel, while drizzle reports accumulated coverage
/// relative to its maximum. Statistical quality is channel-specific because rejection can retain
/// different samples in each RGB channel; monochrome and drizzle quality use shared planes.
#[derive(Debug)]
pub struct StackProduct {
    /// The combined linear image.
    pub image: LinearImage,
    /// Normalized per-pixel coverage in `[0, 1]`, for masking and fill gating.
    pub coverage: Buffer2<f32>,
    /// WHT map. Statistical combines store per-channel sums of surviving frame weights multiplied
    /// by per-pixel confidence; Equal becomes survivor count at unit confidence, while
    /// Noise/Manual normalize frame weights before that multiplier. Drizzle stores one shared
    /// plane of summed geometric drop weights.
    pub weight: QualityMap,
    /// Conditional linear-combine variance factor `Σwᵢ² / (Σwᵢ)²`.
    ///
    /// Present for weighted means and drizzle, using their actual surviving/contributing samples.
    /// `None` for median output because a median is not a linear combination.
    pub linear_variance: Option<QualityMap>,
}
