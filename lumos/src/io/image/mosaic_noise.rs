//! [`MosaicNoise`]: the white noise of each colour of the mosaic a frame was demosaiced from.

use std::array;

use crate::io::image::cfa::CfaType;
use crate::io::image::flat_gain::FlatGain;
use crate::math::noise::difference_noise::DifferenceNoise;
use crate::math::size2us::Size2us;
use crate::math::statistics::median_mut;
use crate::math::statistics::subsample::MAX_STATISTIC_SAMPLES;
use crate::math::vec2us::Vec2us;

/// The white noise of each colour of the mosaic a frame was demosaiced from, measured before the
/// interpolation correlated neighbouring pixels: a measurement of the demosaiced frame reads less
/// noise than its sensor has. PixInsight's Debayer measures its noise estimates at the same point.
///
/// Slot `c` is colour `c` of the pattern, which is channel `c` of the demosaiced frame. The values
/// are in the samples' own units and in the sensor's balance, which the demosaic keeps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MosaicNoise {
    /// Each colour's white-noise standard deviation where no flat amplified it, over the photosites
    /// no flag names.
    pub sigma: [f32; 3],
    /// Each colour's share of `sigma²` that a flat amplified twice, against the share it amplified
    /// once: `σ²·(ρ·g² + (1 − ρ)·g)` at flat gain `g`. 0 for a mosaic no flat divided.
    pub read_share: [f32; 3],
    /// Each colour's median, the level `sigma` was measured at.
    pub sky: [f32; 3],
    /// The mosaic's quantization σ, the floor of `sigma`. The demosaic clears the frame's own.
    pub quantization_sigma: Option<f32>,
}

impl MosaicNoise {
    /// Measured on `mosaic`, a row-major `size` image of a three-colour `cfa_type`, over the
    /// photosites `excluded` does not name, under the flat `gain` divided it by when one did.
    pub(crate) fn measure(
        mosaic: &[f32],
        size: Size2us,
        cfa_type: &CfaType,
        excluded: impl Fn(usize) -> bool,
        quantization_sigma: Option<f32>,
        gain: Option<&FlatGain>,
    ) -> Self {
        assert_eq!(
            cfa_type.num_colors(),
            3,
            "a mosaic of three colours, not {cfa_type:?}"
        );
        let (sigma, read_share) = match gain {
            Some(gain) => {
                let splits =
                    DifferenceNoise::estimate_split(mosaic, size, cfa_type, &excluded, gain);
                (
                    array::from_fn(|colour| splits[colour].variance.sqrt()),
                    array::from_fn(|colour| splits[colour].read_share),
                )
            }
            None => (
                DifferenceNoise::estimate(mosaic, size, cfa_type, &excluded)
                    .into_inner()
                    .expect("one σ per colour"),
                [0.0; 3],
            ),
        };
        Self {
            sigma,
            read_share,
            sky: colour_medians(mosaic, size, cfa_type, excluded),
            quantization_sigma,
        }
    }
}

/// The median of each colour of a mosaic, over every `row_step`-th row so each colour keeps at
/// most about [`MAX_STATISTIC_SAMPLES`] samples, skipping the pixels `excluded` names. A colour
/// with no sample reads 0.
fn colour_medians(
    mosaic: &[f32],
    size: Size2us,
    cfa_type: &CfaType,
    excluded: impl Fn(usize) -> bool,
) -> [f32; 3] {
    let row_step = (size.pixel_count() / (3 * MAX_STATISTIC_SAMPLES)).max(1);
    let mut samples: [Vec<f32>; 3] = Default::default();
    for y in (0..size.height).step_by(row_step) {
        for x in 0..size.width {
            let index = y * size.width + x;
            if !excluded(index) {
                samples[usize::from(cfa_type.color_at(Vec2us::new(x, y)))].push(mosaic[index]);
            }
        }
    }
    samples.map(|mut colour| {
        if colour.is_empty() {
            0.0
        } else {
            median_mut(&mut colour)
        }
    })
}
