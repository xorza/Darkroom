//! Run the display/processing ops on the crate's planar [`crate::LinearImage`] — one
//! `Buffer2<f32>` per channel, the same storage stacking and star detection use.
//!
//! - ops with genuine per-channel 2D structure ([`crate::image_ops::denoise`]'s wavelets,
//!   [`crate::image_ops::background_extraction`]'s surface fit) take their plane straight from the
//!   image and need no adapter at all;
//! - per-pixel ops read the planes in step through the image's own parallel maps: `map_samples`
//!   for work that treats every sample alike, `map_rgb` for work that needs a whole pixel at once
//!   (SCNR, the colour-preserving stretch), and `remap_intensity` for the display enhancers.
//!
//! The submodules below are the image operations themselves (each an op-named config struct with an
//! in-place `apply`), plus their shared support: [`op`] (the `OpError` contract) and [`wavelet`]
//! (the multiscale primitive `denoise`/`hdr` build on).
//!
//! A convention rather than a trait, deliberately. A trait would turn nine inherent `apply`
//! methods into trait methods, so every downstream call site would need it in scope, to save the
//! one-line prologue and express a composability nothing uses — `lens` drives each op from its own
//! node with its own deserialized config type, and would keep doing so. `NeutralizeBackground`
//! takes no parameters and so has no `validate` to call; that is the contract met, not skipped.
//!
//! The two `ml`-gated ops (`MlDenoise`, `RemoveStars`) take the same `apply(&mut LinearImage)`
//! shape but report [`crate::MlError`]: their failures are a missing model or an image smaller
//! than one tile, neither of which is a config range that `InvalidConfigField` describes.

#[cfg(all(test, feature = "internals", feature = "real-data"))]
mod bench;

#[cfg(test)]
mod mem_budget_probe;

pub(crate) mod background_extraction;
pub(crate) mod color_calibration;
pub(crate) mod denoise;
pub(crate) mod error;
pub(crate) mod hdr;
pub(crate) mod local_contrast;
#[cfg(feature = "ml")]
pub(crate) mod ml;
pub(crate) mod rgb;
pub(crate) mod stretching;
pub(crate) mod wavelet;

/// Samples per rayon work item. Parallelizing per sample drowns a cheap per-pixel op in rayon's
/// recursive split/join overhead (it dominated SCNR); a coarse block amortizes that while staying
/// load-balanced and letting the inner loop auto-vectorize.
pub(crate) const SAMPLES_PER_BLOCK: usize = 8192;

#[cfg(test)]
pub(crate) mod internals {
    use crate::io::image::image_dimensions::ImageDimensions;
    use crate::io::image::linear::LinearImage;
    use crate::math::size2us::Size2us;
    use imaginarium::Buffer2;

    pub(crate) fn channel_plane(image: &LinearImage, channel: usize) -> Buffer2<f32> {
        image.channel(channel).clone()
    }

    pub(crate) fn channel_samples(image: &LinearImage, channel: usize) -> Vec<f32> {
        image.channel(channel).pixels().to_vec()
    }

    pub(crate) fn gray_image(size: Size2us, pixels: Vec<f32>) -> LinearImage {
        LinearImage::from(Buffer2::new(size.width, size.height, pixels))
    }

    pub(crate) fn rgb_image(
        size: Size2us,
        red: Vec<f32>,
        green: Vec<f32>,
        blue: Vec<f32>,
    ) -> LinearImage {
        LinearImage::from_planar_channels(
            ImageDimensions::new((size.width, size.height), 3),
            [red, green, blue],
        )
    }

    pub(crate) fn mean(samples: &[f32]) -> f32 {
        assert!(!samples.is_empty());
        samples.iter().sum::<f32>() / samples.len() as f32
    }

    pub(crate) fn standard_deviation(samples: &[f32]) -> f32 {
        let mean = mean(samples);
        (samples
            .iter()
            .map(|&sample| (sample - mean) * (sample - mean))
            .sum::<f32>()
            / samples.len() as f32)
            .sqrt()
    }
}
