//! Image resampling orchestration for registered frames.

use imaginarium::Buffer2;

use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::io::image::linear_pixels::LinearPixels;
use crate::stacking::registration::config::WarpParams;
use crate::stacking::registration::transform::WarpTransform;

#[cfg(all(test, feature = "internals"))]
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
/// The maps are always produced, and by a second pass over the grid rather than alongside the
/// plane warp. Both are deliberate. They are geometry, not pixels, so one pass serves every
/// channel — folding them into the per-channel row loop would either compute them three times for
/// an RGB frame or push its AVX2/NEON kernels back to scalar to carry two more outputs. And the
/// pipeline's only caller needs both, so there is no unconditional cost to skip: `quality::maps`
/// is about two thirds of one plane warp, having been the largest single item in a warp before its
/// interior fast path and tabulated sums.
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
    let mut buffers = WarpBuffers::new(image.dimensions());
    buffers.warp_into(image, warp_transform, config);
    WarpResult {
        image: LinearImage {
            metadata: image.metadata.clone(),
            pixels: buffers.pixels,
        },
        coverage: buffers.coverage,
        confidence: buffers.confidence,
    }
}

/// The three planes a warp fills: the aligned pixels and the two quality maps.
///
/// Every one of them is written in full by [`Self::warp_into`], so a set can be handed straight
/// back for the next frame without clearing. That is the point of naming them: a fresh plane costs
/// its whole area in first-touch page faults, and the spill tier discards its planes once they are
/// on disk, so a warp stage that reuses one set per worker pays that once instead of once per
/// frame. Worth ~22% of a 16 MP warp (`bench_warp_into_fresh_4k` against `..._reused_4k`), or
/// ~0.1 ms per MiB of plane — the faults are spread across rayon's workers, so this is a fraction
/// of what a single thread would pay for the same pages.
#[derive(Debug)]
pub(crate) struct WarpBuffers {
    pub(crate) pixels: LinearPixels,
    pub(crate) coverage: Buffer2<f32>,
    pub(crate) confidence: Buffer2<f32>,
}

impl WarpBuffers {
    pub(crate) fn new(dimensions: ImageDimensions) -> Self {
        Self {
            pixels: LinearPixels::new_zeroed(dimensions),
            coverage: Buffer2::new_default(dimensions.width(), dimensions.height()),
            confidence: Buffer2::new_default(dimensions.width(), dimensions.height()),
        }
    }

    pub(crate) fn dimensions(&self) -> ImageDimensions {
        self.pixels.dimensions()
    }

    /// Warp `image` into these buffers, overwriting all three completely.
    pub(crate) fn warp_into(
        &mut self,
        image: &LinearImage,
        warp_transform: &WarpTransform,
        config: &WarpParams,
    ) {
        // Release assert rather than a `Result`: a non-finite border is caller error, not a runtime
        // failure, and it reaches every pixel outside the source footprint — seeding NaN into the
        // combine, where only a debug assert would notice. Once per frame, so the check is free.
        assert!(
            config.border_value.is_finite(),
            "warp border_value must be finite, got {}",
            config.border_value
        );
        let dimensions = image.dimensions();
        assert_eq!(
            self.dimensions(),
            dimensions,
            "warp buffers were sized for a different frame"
        );

        for channel in 0..dimensions.channels() {
            plane::warp(
                image.channel(channel),
                self.pixels.channel_mut(channel),
                warp_transform,
                config,
            );
        }

        quality::write_maps(
            &mut self.coverage,
            &mut self.confidence,
            dimensions.size(),
            warp_transform,
            config.method,
        );
    }
}

#[cfg(test)]
pub(super) mod internals {
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
