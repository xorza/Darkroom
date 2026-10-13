//! [`MosaicNoise`]: the white noise of each colour of the mosaic a frame was demosaiced from.

use std::array;

use serde::{Deserialize, Serialize};

use crate::frame::stackable_image::StackableImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::quality_flags::QualityFlags;
use crate::math::size2us::Size2us;
use crate::math::statistics;
use crate::math::statistics::subsample::MAX_STATISTIC_SAMPLES;
use crate::noise::difference_noise::DifferenceNoise;
use crate::noise::mosaic_sampling::MosaicSampling;
use imaginarium::Buffer2;

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
    pub(crate) fn heap_bytes(size: Size2us, cfa_type: &CfaType) -> usize {
        let medians: usize = MosaicSampling {
            size,
            cfa_type,
            excluded: |_| false,
            per_colour: MAX_STATISTIC_SAMPLES,
        }
        .photosite_counts()
        .iter()
        .sum();
        DifferenceNoise::heap_bytes(size, cfa_type).max(medians * size_of::<f32>())
    }

    /// Measured on `image`, an undemosaiced mosaic of three colours, over the photosites no flag
    /// names, under the flat gain that divided it when one did.
    pub(crate) fn measure(image: &impl StackableImage) -> Self {
        let cfa_type = image
            .cfa_type()
            .expect("the noise of a mosaic is measured on the mosaic");
        assert_eq!(
            cfa_type.num_colors(),
            3,
            "a mosaic of three colours, not {cfa_type:?}"
        );
        let flags = image.flags();
        let sampling = MosaicSampling {
            size: image.dimensions().size(),
            cfa_type: &cfa_type,
            excluded: |index| flags.is_some_and(|flags| flags.at(index) != QualityFlags::default()),
            per_colour: MAX_STATISTIC_SAMPLES,
        };
        let mosaic = image.channel(0);
        let gain = image.flat_gain();
        let (sigma, read_share) = match gain {
            Some(gain) => {
                let splits = DifferenceNoise::estimate_split(mosaic, &sampling, gain);
                (
                    array::from_fn(|colour| splits[colour].variance.sqrt()),
                    array::from_fn(|colour| splits[colour].read_share),
                )
            }
            None => (
                DifferenceNoise::estimate(mosaic, &sampling)
                    .into_inner()
                    .expect("one σ per colour"),
                [0.0; 3],
            ),
        };
        Self {
            sigma,
            read_share,
            sky: colour_medians(mosaic, &sampling),
            quantization_sigma: image.metadata().quantization_sigma,
        }
    }
}

/// The median of each colour of `mosaic` over the photosites `sampling` takes. A colour with no
/// sample reads 0.
fn colour_medians(
    mosaic: &Buffer2<f32>,
    sampling: &MosaicSampling<'_, impl Fn(usize) -> bool>,
) -> [f32; 3] {
    let mut samples = sampling.photosite_counts().map(Vec::with_capacity);
    sampling.for_each_photosite(|index, colour| samples[colour].push(mosaic[index]));
    samples.map(|mut colour| {
        if colour.is_empty() {
            0.0
        } else {
            statistics::median_mut(&mut colour)
        }
    })
}

#[cfg(test)]
mod tests {
    use imaginarium::Buffer2;

    use crate::image::cfa::cfa_pattern::CfaPattern;
    use crate::image::cfa::cfa_type::CfaType;
    use crate::math::size2us::Size2us;
    use crate::math::vec2us::Vec2us;
    use crate::noise::difference_noise::DifferenceNoise;
    use crate::noise::mosaic_noise::{MosaicNoise, colour_medians};
    use crate::noise::mosaic_sampling::MosaicSampling;

    /// Over an 8 × 4 RGGB mosaic a photosite pairs with the one two to its right, so six pairs a
    /// row, three of each of its two colours: 6 red, 12 green and 6 blue, 24 pairs at 4 bytes and a
    /// bin byte, 120. Its medians take all 32 photosites at 4 bytes, 128, the larger. At 16 × 4,
    /// 14 pairs a row: 56 × 5 = 280 against 64 × 4 = 256.
    #[test]
    fn the_working_set_is_the_larger_of_the_pairs_and_the_medians() {
        let rggb = CfaType::Bayer(CfaPattern::Rggb);
        assert_eq!(DifferenceNoise::heap_bytes(Size2us::new(8, 4), &rggb), 120);
        assert_eq!(MosaicNoise::heap_bytes(Size2us::new(8, 4), &rggb), 128);
        assert_eq!(MosaicNoise::heap_bytes(Size2us::new(16, 4), &rggb), 280);
    }

    /// Each colour's median is its own level when the rows are sampled. At 682 samples a colour a
    /// 64 × 64 RGGB mosaic steps by ⌊4096 / 2046⌋ = 2 periods, so it reads rows 0, 1, 4, 5, …: 16
    /// red rows and 16 blue ones of 32 photosites, 512 each, and 1024 green. A step of 2 single
    /// rows would read only the even rows, and no blue photosite.
    #[test]
    fn a_sampled_mosaic_keeps_every_colour() {
        let size = Size2us::new(64, 64);
        let rggb = CfaType::Bayer(CfaPattern::Rggb);
        let levels = [0.2, 0.1, 0.05];
        let mosaic = Buffer2::from_fn(64, 64, |x, y| levels[rggb.color_at(Vec2us::new(x, y))]);
        let sampling = MosaicSampling {
            size,
            cfa_type: &rggb,
            excluded: |_| false,
            per_colour: 682,
        };
        assert_eq!(sampling.photosite_counts(), [512, 1024, 512]);
        assert_eq!(colour_medians(&mosaic, &sampling), levels);
    }
}
