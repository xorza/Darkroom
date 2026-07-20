//! Image resampling orchestration for registered frames.

use imaginarium::Buffer2;

use crate::AstroImage;
use crate::io::astro_image::PixelData;
use crate::stacking::registration::interpolation::{self, WarpParams};
use crate::stacking::registration::transform::WarpTransform;

/// Output of [`warp`]: the aligned image plus per-pixel support and confidence maps.
///
/// `coverage[p] ∈ [0, 1]` is the fraction of interpolation-kernel magnitude supported by real
/// source pixels. It is a geometric inclusion mask, not a statistical weight. `confidence[p]` is
/// the inverse white-noise variance implied by the normalized interpolation coefficients.
#[derive(Debug)]
pub struct WarpResult {
    pub image: AstroImage,
    pub coverage: Buffer2<f32>,
    pub confidence: Buffer2<f32>,
}

/// Warp an image to align with the reference frame.
///
/// The `WarpTransform` bundles the linear transform with optional SIP distortion
/// correction. Use `result.warp_transform()` to obtain one from a `RegistrationResult`,
/// or `WarpTransform::new(transform)` for a plain transform.
///
/// The output has the same dimensions and metadata as `image`; every output pixel
/// is produced by inverse-mapping, so no input pixels are carried over. Returns a
/// [`WarpResult`] carrying the aligned image and its quality maps (see that type).
///
/// # Arguments
/// * `image` - The source (target) image to warp
/// * `warp_transform` - Combined transform + optional SIP correction
/// * `config` - Configuration for interpolation method
///
/// # Example
///
/// ```ignore
/// use lumos::{RegistrationConfig, register, warp};
///
/// let result = register(&ref_stars, &target_stars, &RegistrationConfig::default())?;
/// let config = RegistrationConfig::default();
/// let aligned = warp(&target_image, &result.warp_transform(), &config.warp).image;
/// ```
pub fn warp(image: &AstroImage, warp_transform: &WarpTransform, config: &WarpParams) -> WarpResult {
    let mut output = AstroImage {
        metadata: image.metadata.clone(),
        dimensions: image.dimensions,
        pixels: PixelData::new_default(image.width(), image.height(), image.channels()),
    };

    for c in 0..image.channels() {
        interpolation::warp_image(
            image.channel(c),
            output.channel_mut(c),
            warp_transform,
            config,
        );
    }

    let quality =
        interpolation::warp_quality_maps(image.dimensions().size(), warp_transform, config.method);

    if config.border_value == 0.0
        && let Some(normalization) = &quality.normalization
    {
        for c in 0..image.channels() {
            renormalize_by_normalization(output.channel_mut(c), normalization);
        }
    }

    WarpResult {
        image: output,
        coverage: quality.coverage,
        confidence: quality.confidence,
    }
}

/// Brighten partially-covered border pixels back to the in-bounds weighted
/// average. Interior pixels (`coverage ≈ 1`) are left bit-exact and fully
/// extrapolated pixels (`coverage ≈ 0`) keep their border value.
fn renormalize_by_normalization(channel: &mut Buffer2<f32>, normalization: &Buffer2<f32>) {
    const PARTIAL_LO: f32 = 1e-4;
    const PARTIAL_HI: f32 = 1.0 - 1e-4;
    for (value, &weight_sum) in channel.pixels_mut().iter_mut().zip(normalization.pixels()) {
        if weight_sum > PARTIAL_LO && weight_sum < PARTIAL_HI {
            *value /= weight_sum;
        }
    }
}
