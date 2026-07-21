//! Image resampling orchestration for registered frames.

use imaginarium::Buffer2;

use crate::LinearImage;
use crate::io::astro_image::PixelData;
use crate::stacking::registration::config::WarpParams;
use crate::stacking::registration::transform::WarpTransform;

#[cfg(test)]
mod bench;
mod kernel;
mod plane;
mod quality;
mod row;
#[cfg(test)]
mod tests;

/// Output of [`warp`]: the aligned image plus per-pixel support and confidence maps.
///
/// `coverage[p] ∈ [0, 1]` is the fraction of interpolation-kernel magnitude supported by real
/// source pixels. It is a geometric inclusion mask, not a statistical weight. `confidence[p]` is
/// the inverse white-noise variance implied by the normalized interpolation coefficients.
#[derive(Debug)]
pub struct WarpResult {
    pub image: LinearImage,
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
pub fn warp(
    image: &LinearImage,
    warp_transform: &WarpTransform,
    config: &WarpParams,
) -> WarpResult {
    let mut output = LinearImage {
        metadata: image.metadata.clone(),
        dimensions: image.dimensions,
        pixels: PixelData::new_default(image.width(), image.height(), image.channels()),
    };

    for c in 0..image.channels() {
        plane::warp(
            image.channel(c),
            output.channel_mut(c),
            warp_transform,
            config,
        );
    }

    let quality = quality::maps(image.dimensions().size(), warp_transform, config.method);

    WarpResult {
        image: output,
        coverage: quality.coverage,
        confidence: quality.confidence,
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use crate::stacking::registration::config::WarpParams;
    use crate::stacking::registration::resample::plane;
    use crate::stacking::registration::transform::WarpTransform;
    use imaginarium::Buffer2;

    pub(crate) fn warp_plane(
        input: &Buffer2<f32>,
        output: &mut Buffer2<f32>,
        transform: &WarpTransform,
        params: &WarpParams,
    ) {
        plane::warp(input, output, transform, params);
    }
}
