use imaginarium::Buffer2;

use crate::io::astro_image::AstroImage;

/// A stacked science product shared by statistical combine and drizzle.
///
/// Each producer documents how it normalizes `coverage`: statistical combine reports the fraction
/// of frames with geometric support at a pixel, while drizzle reports accumulated coverage
/// relative to its maximum. Quality images have the same channel shape as `image`; drizzle's
/// channels are identical because its geometric weights are channel-independent.
#[derive(Debug)]
pub struct StackProduct {
    /// The combined linear image.
    pub image: AstroImage,
    /// Normalized per-pixel coverage in `[0, 1]`, for masking and fill gating.
    pub coverage: Buffer2<f32>,
    /// Per-channel WHT map. Statistical combines sum surviving frame weights multiplied by
    /// per-pixel confidence; Equal becomes survivor count at unit confidence, while Noise/Manual
    /// normalize frame weights before that multiplier. Drizzle sums geometric drop weights.
    pub weight: AstroImage,
    /// Conditional linear-combine variance factor `Σwᵢ² / (Σwᵢ)²`.
    ///
    /// Present for weighted means and drizzle, using their actual surviving/contributing samples.
    /// `None` for median output because a median is not a linear combination.
    pub linear_variance: Option<AstroImage>,
}
