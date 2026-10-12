//! [`NeutralizeBackground`]: each channel's sky shifted to one level, so the sky is neutral grey.

use crate::image_ops::rgb::Rgb;

use crate::image::linear_image::LinearImage;
use crate::image_ops::error::OpError;
use crate::math::statistics::clipped_stats::ClippedStats;
use crate::math::statistics::subsample::Subsample;

/// Sigma-clip parameters for the robust per-channel background estimate (rejects stars/nebula).
const BACKGROUND_KAPPA: f32 = 2.5;
const BACKGROUND_ITERATIONS: usize = 5;

/// Neutralize the per-channel sky background so the background is a neutral gray (R=G=B).
///
/// Estimates each channel's background as a sigma-clipped median, then additively shifts every
/// channel to the darkest channel's level: `IN_x = I_x − BI_x + min(BI_R, BI_G, BI_B)`. A
/// linear-domain operation — run after gradient/background extraction and before the stretch.
/// Additive, so it preserves signal *above* the background (and may push faint pixels slightly
/// negative, which the stretch's black point absorbs). No-op on grayscale.
#[derive(Debug, Clone, Copy, Default)]
pub struct NeutralizeBackground;

impl NeutralizeBackground {
    /// Neutralize `image`'s background in place. A no-op on grayscale, which has no channels to
    /// bring to a common level.
    ///
    /// # Errors
    /// Never — the signature keeps the shape the other ops have, and `lens` drives them uniformly.
    pub fn apply(&self, image: &mut LinearImage) -> Result<(), OpError> {
        if !image.is_rgb() {
            return Ok(());
        }
        let bg = channel_backgrounds(image);
        let target = bg.r.min(bg.g).min(bg.b);
        let (dr, dg, db) = (target - bg.r, target - bg.g, target - bg.b);
        image.map_rgb(move |px| Rgb {
            r: px.r + dr,
            g: px.g + dg,
            b: px.b + db,
        });
        Ok(())
    }
}

/// Per-channel sigma-clipped median background of an RGB image.
pub(super) fn channel_backgrounds(image: &LinearImage) -> Rgb {
    let mut scratch = Vec::new();
    Rgb {
        r: channel_background(image.channel(0), &mut scratch),
        g: channel_background(image.channel(1), &mut scratch),
        b: channel_background(image.channel(2), &mut scratch),
    }
}

/// One channel's robust (sigma-clipped median) background, from a [`Subsample`] of the plane.
fn channel_background(plane: &[f32], scratch: &mut Vec<f32>) -> f32 {
    let mut s = Subsample::statistic_values(plane);
    ClippedStats::sigma_clipped(&mut s, scratch, BACKGROUND_KAPPA, BACKGROUND_ITERATIONS).median
}
