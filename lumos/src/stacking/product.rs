use imaginarium::Buffer2;

use crate::io::astro_image::AstroImage;

/// A stacked science product shared by statistical combine and drizzle.
///
/// Each producer documents how it normalizes `coverage`: statistical combine reports the fraction
/// of frames with geometric support at a pixel, while drizzle reports accumulated coverage
/// relative to its maximum. `weight` and `variance` have the same channel shape as `image`;
/// drizzle's channels are identical because its geometric weights are channel-independent.
#[derive(Debug)]
pub struct StackProduct {
    /// The combined linear image.
    pub image: AstroImage,
    /// Normalized per-pixel coverage in `[0, 1]`, for masking and fill gating.
    pub coverage: Buffer2<f32>,
    /// Per-channel absolute statistical weight `Σwᵢ` (the WHT map).
    pub weight: AstroImage,
    /// Per-channel output variance per unit input variance: `Σwᵢ² / (Σwᵢ)²`.
    pub variance: AstroImage,
}
