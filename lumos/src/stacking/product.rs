use imaginarium::Buffer2;

use crate::io::astro_image::AstroImage;

/// A stacked science product shared by statistical combine and drizzle.
///
/// Each producer documents how it normalizes `coverage`: statistical combine reports the fraction
/// of frames contributing at a pixel, while drizzle reports accumulated coverage relative to its
/// maximum. The weight and variance planes have the same meaning for both producers.
#[derive(Debug)]
pub struct StackProduct {
    /// The combined linear image.
    pub image: AstroImage,
    /// Normalized per-pixel coverage in `[0, 1]`, for masking and fill gating.
    pub coverage: Buffer2<f32>,
    /// Absolute per-pixel statistical weight `Σwᵢ` (the WHT map).
    pub weight: Buffer2<f32>,
    /// Output variance per unit input variance: `Σwᵢ² / (Σwᵢ)²`.
    pub variance: Buffer2<f32>,
}
