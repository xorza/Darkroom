//! Color calibration: neutralize the per-channel sky background and remove the residual green cast
//! from a one-shot-color stack. See `color_calibration/README.md` for the algorithm research.
//!
//! - [`neutralize_background`] (linear, pre-stretch): estimate each channel's background and
//!   additively shift them to a common level, so the sky is neutral gray (R=G=B).
//! - [`scnr`] (post-stretch): Subtractive Chromatic Noise Reduction — clamp green that exceeds the
//!   red/blue average, the residual green being noise on a color-balanced deep-sky image.

use common::Rgb;
use imaginarium::{Buffer2, ChannelCount, Image};
use rayon::prelude::*;

use crate::image_ops::{deinterleave_f32, interleave_f32, par_map_pixels};
use crate::math::statistics::sigma_clipped_median_mad;

#[cfg(test)]
mod tests;

/// Sigma-clip parameters for the robust per-channel background estimate (rejects stars/nebula).
const BACKGROUND_KAPPA: f32 = 2.5;
const BACKGROUND_ITERATIONS: usize = 5;
/// Cap on the per-channel sample size for the background estimate (uniform stride for larger
/// channels, matching `defect_map`'s `MAX_MEDIAN_SAMPLES`). A robust background median converges
/// well below this; small images stay exact (stride 1).
const MAX_BACKGROUND_SAMPLES: usize = 1_000_000;

/// Neutralize the per-channel sky background so the background is a neutral gray (R=G=B).
///
/// Estimates each channel's background as a sigma-clipped median, then additively shifts every
/// channel to the darkest channel's level: `IN_x = I_x − BI_x + min(BI_R, BI_G, BI_B)`. A
/// linear-domain operation — run after gradient/background extraction and before the stretch.
/// Additive, so it preserves signal *above* the background (and may push faint pixels slightly
/// negative, which the stretch's black point absorbs). No-op on grayscale.
pub fn neutralize_background(image: &mut Image) {
    if image.desc.color_format.channel_count != ChannelCount::Rgb {
        return; // no-op on grayscale
    }
    let mut planes = deinterleave_f32(image);
    neutralize_background_core(&mut planes);
    *image = interleave_f32(planes);
}

/// Additively shift each RGB channel to the darkest channel's background level.
fn neutralize_background_core(planes: &mut [Buffer2<f32>]) {
    let bg = channel_backgrounds_planes(planes);
    let target = bg.r.min(bg.g).min(bg.b);
    for (plane, offset) in planes
        .iter_mut()
        .zip([target - bg.r, target - bg.g, target - bg.b])
    {
        plane.pixels_mut().par_iter_mut().for_each(|v| *v += offset);
    }
}

/// One channel's robust (sigma-clipped median) background level. Uniform-stride
/// subsample for speed (exact for small channels).
fn background_level(pixels: &[f32], scratch: &mut Vec<f32>) -> f32 {
    let stride = (pixels.len() / MAX_BACKGROUND_SAMPLES).max(1);
    let mut samples: Vec<f32> = pixels.iter().step_by(stride).copied().collect();
    sigma_clipped_median_mad(
        &mut samples,
        scratch,
        BACKGROUND_KAPPA,
        BACKGROUND_ITERATIONS,
    )
    .median
}

/// Per-channel background color over the RGB channel planes.
fn channel_backgrounds_planes(planes: &[Buffer2<f32>]) -> Rgb {
    let mut scratch = Vec::new();
    Rgb {
        r: background_level(planes[0].pixels(), &mut scratch),
        g: background_level(planes[1].pixels(), &mut scratch),
        b: background_level(planes[2].pixels(), &mut scratch),
    }
}

/// Per-channel background color of an image — a test inspection helper
/// (production goes through [`channel_backgrounds_planes`] on the planes it
/// already deinterleaved).
#[cfg(test)]
pub(crate) fn channel_backgrounds(image: &Image) -> Rgb {
    channel_backgrounds_planes(&deinterleave_f32(image))
}

/// Which SCNR (green-removal) protection method to apply.
#[derive(Debug, Clone, Copy)]
pub enum ScnrMethod {
    /// Average Neutral (the default): `G' = min(G, (R+B)/2)` — a full-strength clamp of green down
    /// to the red/blue average.
    AverageNeutral,
    /// Additive Mask with blend `amount` ∈ [0,1] (0 = no change, 1 = full strength): attenuates
    /// rather than clamps, so genuine teal (OIII planetary nebulae) survives. `m = min(1, R+B)`,
    /// `G' = G·(1−amount)·(1−m) + m·G`.
    AdditiveMask { amount: f32 },
}

/// Remove the residual green cast (Subtractive Chromatic Noise Reduction). Intended for the
/// stretched, already-color-balanced image. No-op on grayscale.
pub fn scnr(image: &mut Image, method: ScnrMethod) {
    if image.desc.color_format.channel_count != ChannelCount::Rgb {
        return; // no-op on grayscale
    }
    match method {
        ScnrMethod::AverageNeutral => par_map_pixels(image, |l| l, scnr_average_neutral),
        ScnrMethod::AdditiveMask { amount } => {
            assert_scnr_amount(amount);
            par_map_pixels(image, |l| l, move |px| scnr_additive_mask(px, amount));
        }
    }
}

fn assert_scnr_amount(amount: f32) {
    assert!(
        (0.0..=1.0).contains(&amount),
        "SCNR amount must be in [0, 1], got {amount}"
    );
}

/// Average Neutral: clamp green to the red/blue average.
fn scnr_average_neutral(px: Rgb) -> Rgb {
    Rgb {
        r: px.r,
        g: px.g.min(0.5 * (px.r + px.b)),
        b: px.b,
    }
}

/// Additive Mask: attenuate green by `amount`, protected where R+B is large.
fn scnr_additive_mask(px: Rgb, amount: f32) -> Rgb {
    let m = (px.r + px.b).min(1.0);
    Rgb {
        r: px.r,
        g: px.g * (1.0 - amount) * (1.0 - m) + m * px.g,
        b: px.b,
    }
}
