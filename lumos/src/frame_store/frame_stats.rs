//! Per-frame robust statistics, measured before any interpolation touches the pixels.

use arrayvec::ArrayVec;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::frame_store::frame_facts::FrameFacts;
use crate::frame_store::stackable_image::StackableImage;
use crate::io::image::cfa::CfaType;
use crate::io::image::pixel_flags::Flags;
use crate::math::noise::ccd_noise::CcdNoise;
use crate::math::noise::difference_noise::DifferenceNoise;
use crate::math::noise::mrs_noise::MrsNoise;
use crate::math::size2us::Size2us;
use crate::math::statistics::subsample::MAX_STATISTIC_SAMPLES;
use crate::math::statistics::{MedianMad, median_mut};
use crate::math::vec2us::Vec2us;

/// Per-frame statistics: one median/MAD pair per channel, and the white noise.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FrameStats {
    pub(crate) channels: ArrayVec<MedianMad, 3>,
    /// The white-noise standard deviation, without the signal MAD includes: per channel by
    /// [`MrsNoise`], or per colour of a mosaic by [`DifferenceNoise`]. Measured over the pixels no
    /// flag names.
    pub(crate) noise: ArrayVec<f32, 3>,
    /// The level `noise` was measured at, in the same slots: each channel's median, or each
    /// colour's median of a mosaic.
    pub(crate) sky: ArrayVec<f32, 3>,
    pub(crate) quantization_sigma: Option<f32>,
    /// From [`CcdNoise::electrons_per_unit`], when the source states its gain.
    pub(crate) electrons_per_unit: Option<f32>,
    /// What the decoder said the samples are; what makes two frames' statistics comparable at all.
    pub(crate) facts: FrameFacts,
}

impl FrameStats {
    /// Measure per-channel median and MAD on `image`, before any interpolation touches it.
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
        let quantization_sigma = image.metadata().quantization_sigma;
        let facts = FrameFacts::of(image);
        let flags = image.flags();
        let excluded =
            |index: usize| flags.is_some_and(|flags| flags.at(index) != Flags::default());
        let mosaic = image
            .cfa_type()
            .filter(|cfa| matches!(cfa, CfaType::Bayer(_) | CfaType::XTrans(_)));
        let noise = match mosaic {
            Some(cfa_type) => {
                DifferenceNoise::estimate(image.channel(0), dimensions.size(), &cfa_type, excluded)
            }
            None => (0..dimensions.channels())
                .into_par_iter()
                .map(|channel| {
                    MrsNoise::estimate(image.channel(channel), dimensions.size(), excluded)
                })
                .collect::<Vec<_>>()
                .into_iter()
                .collect(),
        };
        let nulls = flags.filter(|flags| flags.contains(Flags::NO_DATA));
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
                                    !nulls.at_pos(Vec2us::new(x, y)).intersects(Flags::NO_DATA)
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
        let sky = match mosaic {
            Some(cfa_type) => {
                colour_medians(image.channel(0), dimensions.size(), &cfa_type, excluded)
            }
            None => channels
                .iter()
                .map(|channel: &MedianMad| channel.median)
                .collect(),
        };
        Self {
            channels,
            noise,
            sky,
            quantization_sigma,
            electrons_per_unit: CcdNoise::electrons_per_unit(image.metadata()),
            facts,
        }
    }

    /// The noise model of one slot: a channel, or a colour of a mosaic.
    pub(crate) fn ccd_noise(&self, slot: usize) -> CcdNoise {
        let sigma = self.noise[slot].max(self.quantization_sigma.unwrap_or(0.0));
        CcdNoise {
            background_variance: sigma * sigma,
            sky: self.sky[slot],
            electrons_per_unit: self.electrons_per_unit,
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
) -> ArrayVec<f32, 3> {
    let colours = cfa_type.num_colors();
    let row_step = (size.pixel_count() / (colours * MAX_STATISTIC_SAMPLES)).max(1);
    let mut samples: ArrayVec<Vec<f32>, 3> = (0..colours).map(|_| Vec::new()).collect();
    for y in (0..size.height).step_by(row_step) {
        for x in 0..size.width {
            let index = y * size.width + x;
            if !excluded(index) {
                samples[usize::from(cfa_type.color_at(Vec2us::new(x, y)))].push(mosaic[index]);
            }
        }
    }
    samples
        .into_iter()
        .map(|mut colour| {
            if colour.is_empty() {
                0.0
            } else {
                median_mut(&mut colour)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internals::cfa::make_cfa;
    use crate::io::image::sample_domain::{Pedestal, SampleDomain, ScaleOrigin};
    use crate::io::raw::demosaic::bayer::CfaPattern;

    /// An RGGB mosaic of constant colours, red 1/8, green 1/4, blue 3/8: each colour's sky is its own
    /// level, where the whole-mosaic median, 1/4, would put red's 1/8 below its sky and blue's 3/8
    /// above it. The noise model takes the quantization σ 1/16 where the measured noise is 0, and the
    /// electrons per unit from 2 e⁻/ADU over a declared 1000 ADU: 2000.
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
        assert_eq!(
            stats.ccd_noise(2),
            CcdNoise {
                background_variance: 1.0 / 256.0,
                sky: 0.375,
                electrons_per_unit: Some(2000.0),
            }
        );
    }
}
