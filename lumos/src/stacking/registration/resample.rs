//! Image resampling orchestration for registered frames.

use imaginarium::Buffer2;

use crate::AstroImage;
use crate::io::astro_image::PixelData;
use crate::stacking::registration::config::InterpolationMethod;
use crate::stacking::registration::interpolation::{self, WarpParams};
use crate::stacking::registration::transform::WarpTransform;

/// Output of [`warp`]: the aligned image plus a per-pixel coverage map.
///
/// `coverage[p] ∈ [0, 1]` is the fraction of the interpolation kernel's weight
/// at output pixel `p` that landed on real (in-bounds) source pixels — `1.0`
/// fully inside the source, `0.0` fully extrapolated, fractional along the
/// warped border. Downstream combination (stacking) should feed it as a
/// per-frame pixel weight so extrapolated edges are down-weighted or rejected;
/// without it every warped frame contributes a dark, partially-invented ring
/// to the combined edges.
#[derive(Debug)]
pub struct WarpResult {
    pub image: AstroImage,
    pub coverage: Buffer2<f32>,
}

/// Warp an image to align with the reference frame.
///
/// The `WarpTransform` bundles the linear transform with optional SIP distortion
/// correction. Use `result.warp_transform()` to obtain one from a `RegistrationResult`,
/// or `WarpTransform::new(transform)` for a plain transform.
///
/// The output has the same dimensions and metadata as `image`; every output pixel
/// is produced by inverse-mapping, so no input pixels are carried over. Returns a
/// [`WarpResult`] carrying the aligned image and a coverage map (see that type).
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

    let coverage =
        interpolation::warp_coverage(image.dimensions().size, warp_transform, config.method);

    // Border-flux renormalization: a partially-covered pixel warped with a zero
    // border is `Σ_in(value·w)`; dividing by `coverage = Σ_in(w)` recovers the
    // in-bounds weighted average instead of a value darkened toward the border.
    // Exact only for non-negative kernels (nearest/bilinear); negative-lobe
    // kernels (bicubic/Lanczos) still emit coverage but keep their raw values,
    // and a non-zero border is a deliberate fill we must not rescale.
    if config.border_value == 0.0 && renormalizable(config.method) {
        for c in 0..image.channels() {
            renormalize_by_coverage(output.channel_mut(c), &coverage);
        }
    }

    WarpResult {
        image: output,
        coverage,
    }
}

/// Kernels whose weights are non-negative, where dividing a zero-border warp by
/// its coverage exactly recovers the in-bounds weighted average. Bicubic and
/// Lanczos have negative lobes, so their edge renormalization would need
/// in-sampler weight tracking and is deferred.
fn renormalizable(method: InterpolationMethod) -> bool {
    matches!(
        method,
        InterpolationMethod::Nearest | InterpolationMethod::Bilinear
    )
}

/// Brighten partially-covered border pixels back to the in-bounds weighted
/// average. Interior pixels (`coverage ≈ 1`) are left bit-exact and fully
/// extrapolated pixels (`coverage ≈ 0`) keep their border value.
fn renormalize_by_coverage(channel: &mut Buffer2<f32>, coverage: &Buffer2<f32>) {
    const PARTIAL_LO: f32 = 1e-4;
    const PARTIAL_HI: f32 = 1.0 - 1e-4;
    for (v, &cov) in channel.pixels_mut().iter_mut().zip(coverage.pixels()) {
        if cov > PARTIAL_LO && cov < PARTIAL_HI {
            *v /= cov;
        }
    }
}
