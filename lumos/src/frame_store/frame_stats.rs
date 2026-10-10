//! Per-frame robust statistics, with the white noise measured before any interpolation.

use arrayvec::ArrayVec;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::frame_store::frame_facts::FrameFacts;
use crate::frame_store::stackable_image::StackableImage;
use crate::io::image::cfa::CfaType;
use crate::io::image::mosaic_noise::MosaicNoise;
use crate::io::image::pixel_flags::QualityFlags;
use crate::math::noise::background_split::BackgroundSplit;
use crate::math::noise::ccd_noise::CcdNoise;
use crate::math::noise::mrs_noise::MrsNoise;
use crate::math::statistics::MedianMad;
use crate::math::vec2us::Vec2us;

/// Per-frame statistics: one median/MAD pair per channel, and the white noise.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FrameStats {
    pub(crate) channels: ArrayVec<MedianMad, 3>,
    /// The white-noise standard deviation where no flat amplified it, without the signal MAD
    /// includes: per colour of a mosaic by [`MosaicNoise`], per colour of the mosaic a demosaiced
    /// frame came from, or per channel by [`MrsNoise`]. Measured over the pixels no flag names.
    pub(crate) noise: ArrayVec<f32, 3>,
    /// The share of each slot's `noise²` a flat amplified twice, in the slots of `noise`: see
    /// [`BackgroundSplit`]. 0 for a frame no flat divided.
    pub(crate) read_share: ArrayVec<f32, 3>,
    /// The level `noise` was measured at, in the same slots: each colour's median of the mosaic,
    /// or each channel's median.
    pub(crate) sky: ArrayVec<f32, 3>,
    pub(crate) quantization_sigma: Option<f32>,
    /// From [`CcdNoise::electrons_per_unit`], when the source states its gain.
    pub(crate) electrons_per_unit: Option<f32>,
    /// What the decoder said the samples are; what makes two frames' statistics comparable at all.
    pub(crate) facts: FrameFacts,
}

impl FrameStats {
    /// Measure per-channel median and MAD on `image`, and its white noise before any interpolation:
    /// on a mosaic, per colour; on a demosaiced frame, the [`MosaicNoise`] its demosaic measured,
    /// because the frame's own correlated pixels would understate it; elsewhere, per channel.
    ///
    /// Pixels the source declared no measurement for are left out. They matter most to the MAD: the
    /// decoder fills a null with the frame's own median, so every one of them is a zero-deviation
    /// sample, and a frame with a large masked region would report a spread far below its real
    /// noise — which is the figure weighting divides by.
    ///
    /// The other stages that measure a whole plane need no such exclusion, and the fill is why.
    /// Star detection looks for peaks above a local background and a patch sitting *at* the
    /// background produces none; the defect detectors look for outliers against a median the fill
    /// by construction is; and normalization already re-measures over the pixels every frame shares
    /// once any frame is partially covering.
    pub(crate) fn measure(image: &impl StackableImage) -> Self {
        let dimensions = image.dimensions();
        let metadata = image.metadata();
        let facts = FrameFacts::of(image);
        let flags = image.flags();
        let excluded =
            |index: usize| flags.is_some_and(|flags| flags.at(index) != QualityFlags::default());
        let mosaic = image
            .cfa_type()
            .filter(|cfa| matches!(cfa, CfaType::Bayer(_) | CfaType::XTrans(_)));
        let mosaic_noise = match mosaic {
            Some(cfa_type) => Some(MosaicNoise::measure(
                image.channel(0),
                dimensions.size(),
                &cfa_type,
                excluded,
                metadata.quantization_sigma,
                image.flat_gain(),
            )),
            None => metadata.mosaic_noise.inspect(|_| {
                assert_eq!(
                    dimensions.channels(),
                    3,
                    "a frame's mosaic noise names its three channels"
                );
            }),
        };
        // Each slot's σ where no flat amplified it, and the share of its variance a flat amplified
        // twice.
        let (noise, read_share): (ArrayVec<f32, 3>, ArrayVec<f32, 3>) = match &mosaic_noise {
            Some(mosaic_noise) => (
                ArrayVec::from(mosaic_noise.sigma),
                ArrayVec::from(mosaic_noise.read_share),
            ),
            None => (0..dimensions.channels())
                .into_par_iter()
                .map(|channel| {
                    let plane = image.channel(channel);
                    let size = dimensions.size();
                    match image.flat_gain() {
                        Some(gain) => {
                            let split = MrsNoise::estimate_split(
                                plane,
                                size,
                                excluded,
                                |index| {
                                    gain.at(
                                        channel,
                                        (index % size.width) as f32,
                                        (index / size.width) as f32,
                                    )
                                },
                                gain.bins(channel),
                            );
                            (split.variance.sqrt(), split.read_share)
                        }
                        None => (MrsNoise::estimate(plane, size, excluded), 0.0),
                    }
                })
                .collect::<Vec<_>>()
                .into_iter()
                .unzip(),
        };
        let nulls = flags.filter(|flags| flags.contains(QualityFlags::NO_DATA));
        let channels: ArrayVec<MedianMad, 3> = (0..dimensions.channels())
            .into_par_iter()
            .map(|channel| {
                // One copy per channel, the measured samples, which the median and the MAD then
                // sort in place.
                let plane = image.channel(channel);
                let mut measured: Vec<f32> = match nulls {
                    Some(nulls) => plane
                        .chunks(dimensions.width())
                        .enumerate()
                        .flat_map(|(y, row)| {
                            row.iter()
                                .enumerate()
                                .filter(move |&(x, _)| {
                                    !nulls
                                        .at_pos(Vec2us::new(x, y))
                                        .intersects(QualityFlags::NO_DATA)
                                })
                                .map(|(_, &sample)| sample)
                        })
                        .collect(),
                    None => plane.to_vec(),
                };
                // A frame with nothing measured anywhere has no statistics to report. It also
                // contributes at no pixel, so what goes here is never read — but it has to be
                // something, and the median of nothing would panic.
                if measured.is_empty() {
                    return MedianMad {
                        median: 0.0,
                        mad: 0.0,
                    };
                }
                MedianMad::of_mut(&mut measured)
            })
            .collect::<Vec<_>>()
            .into_iter()
            .collect();
        let sky = match &mosaic_noise {
            Some(mosaic_noise) => ArrayVec::from(mosaic_noise.sky),
            None => channels
                .iter()
                .map(|channel: &MedianMad| channel.median)
                .collect(),
        };
        Self {
            channels,
            noise,
            read_share,
            sky,
            quantization_sigma: match mosaic_noise {
                Some(mosaic_noise) => mosaic_noise.quantization_sigma,
                None => metadata.quantization_sigma,
            },
            electrons_per_unit: CcdNoise::electrons_per_unit(metadata),
            facts,
        }
    }

    /// The white noise of a channel, raised to the quantization σ: the slot's own for a full plane,
    /// and the root mean square over the colours for the one channel of a mosaic.
    pub(crate) fn channel_noise(&self, channel: usize) -> f32 {
        let floor = self.quantization_sigma.unwrap_or(0.0);
        if self.noise.len() == self.channels.len() {
            self.noise[channel].max(floor)
        } else {
            debug_assert_eq!(self.channels.len(), 1);
            let squares: f32 = self
                .noise
                .iter()
                .map(|&sigma| sigma.max(floor).powi(2))
                .sum();
            (squares / self.noise.len() as f32).sqrt()
        }
    }

    /// The noise model of one slot: a channel, or a colour of a mosaic.
    pub(crate) fn ccd_noise(&self, slot: usize) -> CcdNoise {
        let sigma = self.noise[slot];
        let step = self.quantization_sigma.unwrap_or(0.0);
        CcdNoise {
            background: BackgroundSplit {
                variance: sigma * sigma,
                read_share: self.read_share[slot],
            },
            quantization_variance: step * step,
            sky: self.sky[slot],
            electrons_per_unit: self.electrons_per_unit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internals::cfa::make_cfa;
    use crate::io::image::sample_domain::{Pedestal, SampleDomain, ScaleOrigin};
    use crate::io::raw::demosaic::bayer::CfaPattern;
    use crate::math::size2us::Size2us;
    use std::sync::Arc;

    use imaginarium::Buffer2;

    use crate::internals::test_rng::TestRng;

    const SEED: u64 = 7;
    use crate::io::image::flat_gain::FlatGain;
    use crate::io::image::image_dimensions::ImageDimensions;
    use crate::io::image::linear::LinearImage;

    /// An RGGB mosaic of constant colours, red 1/8, green 1/4, blue 3/8: each colour's sky is its
    /// own level, where the whole-mosaic median, 1/4, would put red's 1/8 below its sky and blue's
    /// 3/8 above it. The noise model takes the quantization σ 1/16 where the measured noise is 0,
    /// and the electrons per unit from 2 e⁻/ADU over a declared 1000 ADU: 2000. The mosaic's one
    /// channel has the root mean square of its colours' floored noise, 1/16.
    #[test]
    fn a_mosaic_has_a_sky_per_colour() {
        let size = Size2us::new(8, 8);
        let cfa = CfaType::Bayer(CfaPattern::Rggb);
        let pixels = (0..size.pixel_count())
            .map(
                |index| match cfa.color_at(Vec2us::new(index % 8, index / 8)) {
                    0 => 0.125,
                    1 => 0.25,
                    _ => 0.375,
                },
            )
            .collect();
        let mut image = make_cfa(size, pixels, cfa);
        image.metadata.quantization_sigma = Some(1.0 / 16.0);
        image.metadata.egain = Some(2.0);
        image.metadata.domain = Some(SampleDomain {
            scale: 1000.0,
            origin: ScaleOrigin::Declared,
            pedestal: Pedestal::Removed,
            unit: None,
        });
        let stats = FrameStats::measure(&image);
        assert_eq!(stats.sky.as_slice(), [0.125, 0.25, 0.375]);
        assert_eq!(stats.channels[0].median, 0.25);
        assert_eq!(stats.channel_noise(0), 1.0 / 16.0);
        assert_eq!(
            stats.ccd_noise(2),
            CcdNoise {
                background: BackgroundSplit::unflattened(0.0),
                quantization_variance: 1.0 / 256.0,
                sky: 0.375,
                electrons_per_unit: Some(2000.0),
            }
        );
        assert_eq!(stats.ccd_noise(2).background_at(1.0), 1.0 / 256.0);
    }

    /// A frame whose noise follows `A·g² + S·g` across a flat whose gain rises from 1 to 3 along
    /// its 512 columns measures the two terms back: `σ²` = A + S and `ρ` = A/(A + S). A mono frame
    /// 256 rows tall is read by the multiresolution support over two 256² tiles, 16 k coefficients
    /// a bin, whose variances carry a standard error of √(2/16k) = 1.1%; the fit extrapolates to
    /// gain 1 from the lowest bin near 1.1, so 6% and 0.06 hold `σ²` and `ρ` at about five standard
    /// errors. A 512² mosaic's colours are read from pairs of neighbours, 8 k a bin for red and
    /// blue, whose MAD-based variance carries about twice that error, so 10% and 0.2. All read
    /// noise and all sky are told apart: `ρ` at 1 and at 0.
    #[test]
    fn a_flat_divided_frame_splits_its_noise_by_the_flat_gain() {
        let gain = |x: usize| 1.0 + 2.0 * x as f64 / 511.0;
        let frame = |size: Size2us, read: f64, sky: f64| {
            let mut rng = TestRng::new(SEED);
            let pixels: Vec<f32> = (0..size.pixel_count())
                .map(|index| {
                    let g = gain(index % size.width);
                    let sigma = (read * g * g + sky * g).sqrt();
                    (0.5 + sigma * f64::from(rng.next_gaussian_f32())) as f32
                })
                .collect();
            let divisor = Buffer2::new(
                size.width,
                size.height,
                (0..size.pixel_count())
                    .map(|index| (1.0 / gain(index % size.width)) as f32)
                    .collect(),
            );
            (pixels, divisor)
        };
        let cfa = CfaType::Bayer(CfaPattern::Rggb);
        for (read, sky) in [(1e-4f64, 1e-4f64), (2e-4, 0.0), (0.0, 2e-4)] {
            let expected_share = (read / (read + sky)) as f32;
            let check = |stats: FrameStats, variance_tolerance: f32, share_tolerance: f32| {
                for (slot, (&sigma, &share)) in
                    stats.noise.iter().zip(&stats.read_share).enumerate()
                {
                    let variance = sigma * sigma;
                    assert!(
                        (variance / 2e-4 - 1.0).abs() <= variance_tolerance,
                        "A {read} S {sky} slot {slot}: σ² {variance}"
                    );
                    assert!(
                        (share - expected_share).abs() <= share_tolerance,
                        "A {read} S {sky} slot {slot}: ρ {share}"
                    );
                }
            };
            let size = Size2us::new(512, 256);
            let (pixels, divisor) = frame(size, read, sky);
            let mut mono = LinearImage::from_pixels(ImageDimensions::new(size, 1), pixels);
            mono.metadata.flat_gain = Some(Arc::new(FlatGain::of_divisor(
                &divisor,
                &CfaType::Mono,
                |_| false,
            )));
            check(FrameStats::measure(&mono), 0.06, 0.06);
            let size = Size2us::new(512, 512);
            let (pixels, divisor) = frame(size, read, sky);
            let mut mosaic = make_cfa(size, pixels, cfa);
            mosaic.metadata.flat_gain =
                Some(Arc::new(FlatGain::of_divisor(&divisor, &cfa, |_| false)));
            check(FrameStats::measure(&mosaic), 0.1, 0.2);
        }
    }
}
