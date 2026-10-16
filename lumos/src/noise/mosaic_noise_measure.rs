//! [`MosaicNoiseMeasure`]: a mosaic's noise, measured before its demosaic.

use std::sync::Arc;

use imaginarium::Buffer2;

use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::flat_gain::FlatGain;
use crate::image::mosaic_noise::MosaicNoise;
use crate::image::pixel_flags::PixelFlags;
use crate::math::size2us::Size2us;
use crate::math::statistics;
use crate::math::statistics::subsample::MAX_STATISTIC_SAMPLES;
use crate::noise::mosaic_sampling::MosaicSampling;

/// An undemosaiced mosaic of three colors, as its noise is measured: its plane under its
/// pattern, the flags that leave photosites out, the flat gain that divided it when one did, and
/// its quantization σ, the floor of every color's.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MosaicNoiseMeasure<'a> {
    pub(crate) plane: &'a Buffer2<f32>,
    pub(crate) cfa_type: CfaType,
    pub(crate) flags: Option<&'a PixelFlags>,
    pub(crate) flat_gain: Option<&'a FlatGain>,
    pub(crate) quantization_sigma: Option<f32>,
}

impl<'a> MosaicNoiseMeasure<'a> {
    /// `mosaic` as its noise is measured; `None` for a mono frame, which has no colors to tell
    /// apart and keeps its own noise through a demosaic.
    pub(crate) fn of(mosaic: &'a CfaImage) -> Option<Self> {
        mosaic.cfa_type().is_mosaic().then(|| Self {
            plane: mosaic.data(),
            cfa_type: mosaic.cfa_type(),
            flags: mosaic.flags(),
            flat_gain: mosaic.flat_gain().map(Arc::as_ref),
            quantization_sigma: mosaic.metadata.derived.quantization_sigma(),
        })
    }

    /// The most bytes [`Self::measure`] holds over a mosaic of `size` and the three-color
    /// `cfa_type`: the larger of the pairs its noise is measured over and the samples its medians
    /// are, which it holds one after the other.
    pub(crate) fn heap_bytes(size: Size2us, cfa_type: &CfaType) -> usize {
        let sampling = MosaicSampling {
            size,
            cfa_type,
            excluded: |_| false,
            per_color: MAX_STATISTIC_SAMPLES,
        };
        let medians: usize = sampling.photosite_counts().iter().sum();
        sampling.noise_heap_bytes().max(medians * size_of::<f32>())
    }

    /// Measured over the photosites no flag names, under the flat gain when there is one.
    pub(crate) fn measure(self) -> MosaicNoise {
        let Self {
            plane: mosaic,
            cfa_type,
            flags,
            flat_gain,
            quantization_sigma,
        } = self;
        assert_eq!(
            cfa_type.num_colors(),
            3,
            "a mosaic of three colors, not {cfa_type:?}"
        );
        let sampling = MosaicSampling {
            size: Size2us::from(mosaic),
            cfa_type: &cfa_type,
            excluded: |index| flags.is_some_and(|flags| flags.is_flagged(index)),
            per_color: MAX_STATISTIC_SAMPLES,
        };
        let background = sampling.backgrounds(mosaic, flat_gain);
        MosaicNoise {
            background: background.into_inner().expect("a split per color"),
            sky: color_medians(mosaic, &sampling),
            quantization_sigma,
        }
    }
}

/// The median of each color of `mosaic` over the photosites `sampling` takes. A color with no
/// sample reads 0.
fn color_medians(
    mosaic: &Buffer2<f32>,
    sampling: &MosaicSampling<'_, impl Fn(usize) -> bool>,
) -> [f32; 3] {
    let mut samples = sampling.photosite_counts().map(Vec::with_capacity);
    sampling.for_each_photosite(|index, color| samples[color].push(mosaic[index]));
    samples.map(|mut color| {
        if color.is_empty() {
            0.0
        } else {
            statistics::median_mut(&mut color)
        }
    })
}

#[cfg(test)]
mod tests {
    use imaginarium::Buffer2;

    use crate::image::cfa::cfa_pattern::CfaPattern;
    use crate::image::cfa::cfa_type::CfaType;
    use crate::math::size2us::Size2us;
    use crate::math::statistics::subsample::MAX_STATISTIC_SAMPLES;
    use crate::math::vec2us::Vec2us;
    use crate::noise::mosaic_noise_measure::{MosaicNoiseMeasure, color_medians};
    use crate::noise::mosaic_sampling::MosaicSampling;

    /// Over an 8 × 4 RGGB mosaic a photosite pairs with the one two to its right, so six pairs a
    /// row, three of each of its two colors: 6 red, 12 green and 6 blue, 24 pairs at 4 bytes and a
    /// bin byte, 120. Its medians take all 32 photosites at 4 bytes, 128, the larger. At 16 × 4,
    /// 14 pairs a row: 56 × 5 = 280 against 64 × 4 = 256.
    #[test]
    fn the_working_set_is_the_larger_of_the_pairs_and_the_medians() {
        let rggb = CfaType::Bayer(CfaPattern::Rggb);
        assert_eq!(
            MosaicSampling {
                size: Size2us::new(8, 4),
                cfa_type: &rggb,
                excluded: |_| false,
                per_color: MAX_STATISTIC_SAMPLES,
            }
            .noise_heap_bytes(),
            120
        );
        assert_eq!(
            MosaicNoiseMeasure::heap_bytes(Size2us::new(8, 4), &rggb),
            128
        );
        assert_eq!(
            MosaicNoiseMeasure::heap_bytes(Size2us::new(16, 4), &rggb),
            280
        );
    }

    /// Each color's median is its own level when the rows are sampled. At 682 samples a color a
    /// 64 × 64 RGGB mosaic steps by ⌊4096 / 2046⌋ = 2 periods, so it reads rows 0, 1, 4, 5, …: 16
    /// red rows and 16 blue ones of 32 photosites, 512 each, and 1024 green. A step of 2 single
    /// rows would read only the even rows, and no blue photosite.
    #[test]
    fn a_sampled_mosaic_keeps_every_color() {
        let size = Size2us::new(64, 64);
        let rggb = CfaType::Bayer(CfaPattern::Rggb);
        let levels = [0.2, 0.1, 0.05];
        let mosaic = Buffer2::from_fn(64, 64, |x, y| levels[rggb.slot_at(Vec2us::new(x, y))]);
        let sampling = MosaicSampling {
            size,
            cfa_type: &rggb,
            excluded: |_| false,
            per_color: 682,
        };
        assert_eq!(sampling.photosite_counts(), [512, 1024, 512]);
        assert_eq!(color_medians(&mosaic, &sampling), levels);
    }
}
