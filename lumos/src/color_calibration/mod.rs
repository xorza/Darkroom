//! Color calibration: neutralize the per-channel sky background and remove the residual green cast
//! from a one-shot-color stack. See `color_calibration/README.md` for the algorithm research.
//!
//! - [`neutralize_background`] (linear, pre-stretch): estimate each channel's background and
//!   additively shift them to a common level, so the sky is neutral gray (R=G=B).
//! - [`scnr`] (post-stretch): Subtractive Chromatic Noise Reduction — clamp green that exceeds the
//!   red/blue average, the residual green being noise on a color-balanced deep-sky image.

use common::Rgb;
use imaginarium::Image;
use rayon::prelude::*;

use crate::io::astro_image::AstroImage;
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
    let mut astro = AstroImage::from(&*image);
    neutralize_background_planar(&mut astro);
    *image = Image::from(&astro);
}

pub(crate) fn neutralize_background_planar(image: &mut AstroImage) {
    if !image.is_rgb() {
        return;
    }
    let bg = channel_backgrounds(image);
    let target = bg.r.min(bg.g).min(bg.b);
    for (c, offset) in [target - bg.r, target - bg.g, target - bg.b]
        .into_iter()
        .enumerate()
    {
        image
            .channel_mut(c)
            .pixels_mut()
            .par_iter_mut()
            .for_each(|v| *v += offset);
    }
}

/// Per-channel robust (sigma-clipped median) background level — i.e. the background's color.
pub(crate) fn channel_backgrounds(image: &AstroImage) -> Rgb {
    let mut scratch = Vec::new();
    let mut median = |c: usize| {
        // Uniform-stride subsample for a fast robust background level (exact for small channels).
        let pixels = image.channel(c).pixels();
        let stride = (pixels.len() / MAX_BACKGROUND_SAMPLES).max(1);
        let mut samples: Vec<f32> = pixels.iter().step_by(stride).copied().collect();
        sigma_clipped_median_mad(
            &mut samples,
            &mut scratch,
            BACKGROUND_KAPPA,
            BACKGROUND_ITERATIONS,
        )
        .median
    };
    Rgb {
        r: median(0),
        g: median(1),
        b: median(2),
    }
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
    let mut astro = AstroImage::from(&*image);
    scnr_planar(&mut astro, method);
    *image = Image::from(&astro);
}

pub(crate) fn scnr_planar(image: &mut AstroImage, method: ScnrMethod) {
    if !image.is_rgb() {
        return;
    }
    match method {
        ScnrMethod::AverageNeutral => image.par_map_pixels(
            |l| l,
            |px| Rgb {
                r: px.r,
                g: px.g.min(0.5 * (px.r + px.b)),
                b: px.b,
            },
        ),
        ScnrMethod::AdditiveMask { amount } => {
            assert!(
                (0.0..=1.0).contains(&amount),
                "SCNR amount must be in [0, 1], got {amount}"
            );
            image.par_map_pixels(
                |l| l,
                move |px| {
                    let m = (px.r + px.b).min(1.0);
                    Rgb {
                        r: px.r,
                        g: px.g * (1.0 - amount) * (1.0 - m) + m * px.g,
                        b: px.b,
                    }
                },
            );
        }
    }
}
