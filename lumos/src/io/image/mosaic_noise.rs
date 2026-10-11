//! [`MosaicNoise`]: the white noise of each colour of the mosaic a frame was demosaiced from.

use std::array;

use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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
    /// The most bytes [`Self::measure`] holds over a mosaic of `size` and the three-colour
    /// `cfa_type`: the larger of the pairs its noise is measured over and the samples its medians
    /// are, which it holds one after the other.
    pub(crate) fn peak_bytes(size: Size2us, cfa_type: &CfaType) -> usize {
        let medians: usize = sample_counts(size, cfa_type, &|_| false).iter().sum();
        DifferenceNoise::peak_bytes(size, cfa_type).max(medians * size_of::<f32>())
    }

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
    let mut samples = sample_counts(size, cfa_type, &excluded).map(Vec::with_capacity);
    for_each_sample(size, cfa_type, &excluded, |index, colour| {
        samples[colour].push(mosaic[index]);
    });
    samples.map(|mut colour| {
        if colour.is_empty() {
            0.0
        } else {
            median_mut(&mut colour)
        }
    })
}

/// How many samples of each colour [`colour_medians`] takes under `excluded`.
fn sample_counts(
    size: Size2us,
    cfa_type: &CfaType,
    excluded: &impl Fn(usize) -> bool,
) -> [usize; 3] {
    let mut counts = [0; 3];
    for_each_sample(size, cfa_type, excluded, |_, colour| counts[colour] += 1);
    counts
}

/// The index and colour of every photosite of every `row_step`-th row that `excluded` does not
/// name.
fn for_each_sample(
    size: Size2us,
    cfa_type: &CfaType,
    excluded: &impl Fn(usize) -> bool,
    mut visit: impl FnMut(usize, usize),
) {
    let row_step = (size.pixel_count() / (3 * MAX_STATISTIC_SAMPLES)).max(1);
    for y in (0..size.height).step_by(row_step) {
        for x in 0..size.width {
            let index = y * size.width + x;
            if !excluded(index) {
                visit(index, cfa_type.color_at(Vec2us::new(x, y)));
            }
        }
    }
}
