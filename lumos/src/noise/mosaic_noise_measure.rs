//! [`MosaicNoiseMeasure`]: a mosaic's noise, measured before its demosaic.

use common::Buffer2;

use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::flat_gain::FlatGain;
use crate::image::metadata::mosaic_noise::MosaicNoise;
use crate::math::statistics;
use crate::math::statistics::subsample::MAX_STATISTIC_SAMPLES;
use crate::noise::mosaic_sampling::MosaicSampling;
use common::USize2;

/// An undemosaiced mosaic of three colors, as its noise is measured: its plane under its
/// pattern, the flags that leave photosites out, the flat gain that divided it when one did, and
/// its quantization σ, the floor of every color's.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MosaicNoiseMeasure<'a> {
    pub(crate) plane: &'a Buffer2<f32>,
    pub(crate) mosaic: Mosaic,
    pub(crate) flags: Option<&'a PixelFlags>,
    pub(crate) flat_gain: Option<&'a FlatGain>,
    pub(crate) quantization_sigma: Option<f32>,
}

impl<'a> MosaicNoiseMeasure<'a> {
    /// `image` as its noise is measured; `None` for a mono frame, which has no colors to tell
    /// apart and keeps its own noise through a demosaic.
    pub(crate) fn of(image: &'a CfaImage) -> Option<Self> {
        Some(Self {
            plane: image.data(),
            mosaic: image.cfa_type().mosaic()?,
            flags: image.flags(),
            flat_gain: image.flat_gain().map(|gain| &**gain),
            quantization_sigma: image.metadata.derived.quantization_sigma(),
        })
    }

    /// The most bytes [`Self::measure`] holds over a `size` frame of `mosaic`: the larger of the pairs its noise is measured over and the samples its medians
    /// are, which it holds one after the other.
    pub(crate) fn heap_bytes(size: USize2, mosaic: Mosaic) -> usize {
        let sampling = MosaicSampling {
            size,
            mosaic,
            excluded: |_| false,
            per_color: MAX_STATISTIC_SAMPLES,
        };
        let medians: usize = sampling.photosite_counts().iter().sum();
        sampling.noise_heap_bytes().max(medians * size_of::<f32>())
    }

    /// Measured over the photosites no flag names, under the flat gain when there is one.
    pub(crate) fn measure(self) -> MosaicNoise {
        let Self {
            plane,
            mosaic,
            flags,
            flat_gain,
            quantization_sigma,
        } = self;
        let sampling = MosaicSampling {
            size: plane.size(),
            mosaic,
            excluded: |idx| flags.is_some_and(|flags| flags.is_flagged(idx)),
            per_color: MAX_STATISTIC_SAMPLES,
        };
        let bg = sampling.bgs(plane, flat_gain);
        MosaicNoise {
            bg: bg.into_inner().expect("a split per color"),
            sky: color_medians(plane, &sampling),
            quantization_sigma,
        }
    }
}

/// The median of each color of `mosaic` over the photosites `sampling` takes. A color with no
/// sample reads 0.
fn color_medians(
    mosaic: &Buffer2<f32>,
    sampling: &MosaicSampling<impl Fn(usize) -> bool>,
) -> [f32; 3] {
    let mut samples = sampling.photosite_counts().map(Vec::with_capacity);
    sampling.for_each_photosite(|idx, color| samples[color].push(mosaic[idx]));
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
    use common::Buffer2;

    use crate::image::cfa::bayer_pattern::BayerPattern;
    use crate::image::cfa::mosaic::Mosaic;
    use crate::math::statistics::subsample::MAX_STATISTIC_SAMPLES;
    use crate::noise::mosaic_noise_measure::{MosaicNoiseMeasure, color_medians};
    use crate::noise::mosaic_sampling::MosaicSampling;
    use common::USize2;

    /// Over an 8 × 4 RGGB mosaic a photosite pairs with the one two to its right, so six pairs a
    /// row, three of each of its two colors: 6 red, 12 green and 6 blue, 24 pairs at 4 bytes and a
    /// bin byte, 120. Its medians take all 32 photosites at 4 bytes, 128, the larger. At 16 × 4,
    /// 14 pairs a row: 56 × 5 = 280 against 64 × 4 = 256.
    #[test]
    fn the_working_set_is_the_larger_of_the_pairs_and_the_medians() {
        let rggb = Mosaic::Bayer(BayerPattern::Rggb);
        assert_eq!(
            MosaicSampling {
                size: USize2::new(8, 4),
                mosaic: rggb,
                excluded: |_| false,
                per_color: MAX_STATISTIC_SAMPLES,
            }
            .noise_heap_bytes(),
            120
        );
        assert_eq!(MosaicNoiseMeasure::heap_bytes(USize2::new(8, 4), rggb), 128);
        assert_eq!(
            MosaicNoiseMeasure::heap_bytes(USize2::new(16, 4), rggb),
            280
        );
    }

    /// Each color's median is its own level when the rows are sampled. At 682 samples a color a
    /// 64 × 64 RGGB mosaic steps by ⌊4096 / 2046⌋ = 2 periods, so it reads rows 0, 1, 4, 5, …: 16
    /// red rows and 16 blue ones of 32 photosites, 512 each, and 1024 green. A step of 2 single
    /// rows would read only the even rows, and no blue photosite.
    #[test]
    fn a_sampled_mosaic_keeps_every_color() {
        let size = USize2::new(64, 64);
        let rggb = Mosaic::Bayer(BayerPattern::Rggb);
        let levels = [0.2, 0.1, 0.05];
        let mosaic = Buffer2::from_fn((64, 64), |at| levels[rggb.slot_at(at)]);
        let sampling = MosaicSampling {
            size,
            mosaic: rggb,
            excluded: |_| false,
            per_color: 682,
        };
        assert_eq!(sampling.photosite_counts(), [512, 1024, 512]);
        assert_eq!(color_medians(&mosaic, &sampling), levels);
    }
}
