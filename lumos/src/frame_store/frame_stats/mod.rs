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
use crate::math::statistics;
use crate::math::vec2us::Vec2us;

/// Per-frame statistics, per slot: a channel, or a colour of a mosaic, whose one channel holds three
/// colours of their own level and noise.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FrameStats {
    /// Each slot's median over its pixels the source measured.
    pub(crate) medians: ArrayVec<f32, 3>,
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
    /// Measure each slot's median on `image`, and its white noise before any interpolation: on a
    /// mosaic, per colour; on a demosaiced frame, the [`MosaicNoise`] its demosaic measured, because
    /// the frame's own correlated pixels would understate it; elsewhere, per channel.
    ///
    /// Pixels the source declared no measurement for are left out: the decoder fills a null with
    /// the frame's own median, so a frame with a large masked region would otherwise report the
    /// fill's level.
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
        let mosaic = image.cfa_type().filter(CfaType::is_mosaic);
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
        let measured = |index: usize| {
            nulls.is_none_or(|nulls| !nulls.at(index).intersects(QualityFlags::NO_DATA))
        };
        let width = dimensions.width();
        let medians: ArrayVec<f32, 3> = match mosaic {
            Some(cfa_type) => (0..cfa_type.num_colors())
                .into_par_iter()
                .map(|colour| {
                    slot_median(image.channel(0), width, |index, position| {
                        measured(index) && usize::from(cfa_type.color_at(position)) == colour
                    })
                })
                .collect::<Vec<_>>(),
            None => (0..dimensions.channels())
                .into_par_iter()
                .map(|channel| {
                    slot_median(image.channel(channel), width, |index, _| measured(index))
                })
                .collect::<Vec<_>>(),
        }
        .into_iter()
        .collect();
        let sky = match &mosaic_noise {
            Some(mosaic_noise) => ArrayVec::from(mosaic_noise.sky),
            None => medians.clone(),
        };
        Self {
            medians,
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

    /// The statistics of a frame as those of the image its drizzle makes of it, which is no
    /// sensor frame: a mosaic's colour slots become the channels of their colours, as for the
    /// frame its demosaic makes, and nothing is a mosaic any more.
    pub(crate) const fn into_drizzled(mut self) -> Self {
        self.facts.cfa_type = None;
        self
    }

    /// The white noise of a slot, raised to the quantization σ.
    pub(crate) fn slot_noise(&self, slot: usize) -> f32 {
        self.noise[slot].max(self.quantization_sigma.unwrap_or(0.0))
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

/// The median of the samples of `plane`, rows `width` long, whose index and position `keep` names;
/// 0 when it names none, a frame with nothing measured there, which then contributes at none of
/// them, so the figure is never read — but the median of nothing would panic.
fn slot_median(plane: &[f32], width: usize, keep: impl Fn(usize, Vec2us) -> bool) -> f32 {
    let mut samples: Vec<f32> = plane
        .chunks(width)
        .enumerate()
        .flat_map(|(y, row)| {
            let keep = &keep;
            row.iter()
                .enumerate()
                .filter(move |&(x, _)| keep(y * width + x, Vec2us::new(x, y)))
                .map(|(_, &sample)| sample)
        })
        .collect();
    if samples.is_empty() {
        return 0.0;
    }
    statistics::median_mut(&mut samples)
}

#[cfg(test)]
mod tests;
