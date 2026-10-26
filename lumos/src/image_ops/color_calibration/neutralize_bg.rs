//! [`NeutralizeBg`]: each channel's sky shifted to one level, so the sky is neutral grey.

use crate::image::linear_image::LinearImage;
use crate::image::rgb::Rgb;
use crate::image_ops::error::OpError;
use crate::math::statistics::clipped_stats::{ClippedStats, SigmaClip};

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
    /// [`OpError::NoData`] when every pixel of the image holds a fill.
    pub fn apply(&self, image: &mut LinearImage) -> Result<(), OpError> {
        if !image.is_rgb() {
            return Ok(());
        }
        let bg = channel_bgs(image)?;
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

/// Per-channel sigma-clipped median background of an RGB image, from the pixels holding data.
pub(super) fn channel_bgs(image: &LinearImage) -> Result<Rgb, OpError> {
    let mut scratch = Vec::new();
    let mut bg = |channel| {
        let mut values = image.statistic_values(channel);
        if values.is_empty() {
            return Err(OpError::NoData);
        }
        Ok(ClippedStats::sigma_clipped(
            &mut values,
            &mut scratch,
            SigmaClip {
                kappa: BG_KAPPA,
                iterations: BG_ITERATIONS,
            },
        )
        .median)
    };
    Ok(Rgb {
        r: bg(0)?,
        g: bg(1)?,
        b: bg(2)?,
    })
}
