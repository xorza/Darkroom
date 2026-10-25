//! [`NeutralizeBg`]: each channel's sky shifted to one level, so the sky is neutral grey.

use crate::image::linear_image::LinearImage;
use crate::image::rgb::Rgb;
use crate::image_ops::error::OpError;
use crate::math::statistics::clipped_stats::{ClippedStats, SigmaClip};
use crate::math::statistics::subsample::Subsample;

/// Sigma-clip parameters for the robust per-channel background estimate (rejects stars/nebula).
const BG_KAPPA: f32 = 2.5;
const BG_ITERATIONS: usize = 5;

/// Neutralize the per-channel sky background so the background is a neutral gray (R=G=B).
///
/// Estimates each channel's background as a sigma-clipped median, then additively shifts every
/// channel to the darkest channel's level: `IN_x = I_x − BI_x + min(BI_R, BI_G, BI_B)`. A
/// linear-domain operation — run after gradient/background extraction and before the stretch.
/// Additive, so it preserves signal *above* the background (and may push faint pixels slightly
/// negative, which the stretch's black point absorbs). No-op on mono.
#[derive(Debug, Clone, Copy, Default)]
pub struct NeutralizeBg;

impl NeutralizeBg {
    /// Neutralize `image`'s background in place. A no-op on mono, which has no channels to
    /// bring to a common level.
    ///
    /// # Errors
    /// Never — the signature keeps the shape the other ops have, and `lens` drives them uniformly.
    pub fn apply(&self, image: &mut LinearImage) -> Result<(), OpError> {
        if !image.is_rgb() {
            return Ok(());
        }
        let bg = channel_bgs(image);
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
pub(super) fn channel_bgs(image: &LinearImage) -> Rgb {
    let mut scratch = Vec::new();
    Rgb {
        r: channel_bg(image.channel(0), &mut scratch),
        g: channel_bg(image.channel(1), &mut scratch),
        b: channel_bg(image.channel(2), &mut scratch),
    }
}

/// One channel's robust (sigma-clipped median) background, from a [`Subsample`] of the plane.
fn channel_bg(plane: &[f32], scratch: &mut Vec<f32>) -> f32 {
    let mut s = Subsample::statistic_values(plane);
    ClippedStats::sigma_clipped(
        &mut s,
        scratch,
        SigmaClip {
            kappa: BG_KAPPA,
            iterations: BG_ITERATIONS,
        },
    )
    .median
}
