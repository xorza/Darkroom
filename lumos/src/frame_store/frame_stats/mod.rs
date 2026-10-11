//! Per-frame robust statistics, with the white noise measured before any interpolation.

use arrayvec::ArrayVec;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::frame_store::frame_facts::FrameFacts;
use crate::frame_store::stackable_image::StackableImage;
use crate::io::image::cfa::CfaType;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::mosaic_noise::MosaicNoise;
use crate::io::image::pixel_flags::QualityFlags;
use crate::math::noise::background_split::BackgroundSplit;
use crate::math::noise::ccd_noise::CcdNoise;
use crate::math::noise::mrs_noise::MrsNoise;
use crate::math::statistics;
use crate::math::statistics::median_mad::MedianMad;
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
    /// Each slot's robust spread over the same pixels as its median: noise and signal together.
    /// Measured only where a normalization scales frames one by one, which a drizzle of mosaics
    /// does ([`Self::measure_with_spread`]); `None` elsewhere.
    pub(crate) spread: Option<ArrayVec<SlotSpread, 3>>,
    pub(crate) quantization_sigma: Option<f32>,
    /// From [`ImageMetadata::electrons_per_unit`], when the source states its gain.
    ///
    /// [`ImageMetadata::electrons_per_unit`]: crate::io::image::image_metadata::ImageMetadata::electrons_per_unit
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
        Self::measured(image, false)
    }

    /// [`Self::measure`], and each slot's [`spread`](Self::spread) from the samples its median
    /// already took.
    pub(crate) fn measure_with_spread(image: &impl StackableImage) -> Self {
        Self::measured(image, true)
    }

    /// The most bytes [`Self::measure`] holds beside a frame of `dimensions` with `threads`
    /// workers: its noise first — a mosaic's by [`MosaicNoise`], any other frame's by [`MrsNoise`]
    /// one channel at a time — then the samples its levels sort, every slot's at once.
    pub(crate) fn heap_bytes(
        dimensions: ImageDimensions,
        mosaic: Option<&CfaType>,
        threads: usize,
    ) -> usize {
        let size = dimensions.size();
        match mosaic {
            Some(cfa_type) => MosaicNoise::heap_bytes(size, cfa_type)
                .max(dimensions.pixel_count() * size_of::<f32>()),
            None => MrsNoise::heap_bytes(size, threads).max(dimensions.frame_bytes()),
        }
    }

    fn measured(image: &impl StackableImage, with_spread: bool) -> Self {
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
            // One channel at a time: each estimate spreads its tiles over the workers already, and
            // the plan charges one estimate's coefficients.
            None => (0..dimensions.channels())
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
                .unzip(),
        };
        let nulls = flags.filter(|flags| flags.contains(QualityFlags::NO_DATA));
        let measured = |index: usize| {
            nulls.is_none_or(|nulls| !nulls.at(index).intersects(QualityFlags::NO_DATA))
        };
        let width = dimensions.width();
        let levels: Vec<SlotLevel> = match mosaic {
            Some(cfa_type) => (0..cfa_type.num_colors())
                .into_par_iter()
                .map(|colour| {
                    SlotLevel::of(image.channel(0), width, with_spread, |index, position| {
                        measured(index) && cfa_type.color_at(position) == colour
                    })
                })
                .collect(),
            // One channel at a time: each estimate spreads its tiles over the workers already, and
            // the plan charges one estimate's coefficients.
            None => (0..dimensions.channels())
                .map(|channel| {
                    SlotLevel::of(image.channel(channel), width, with_spread, |index, _| {
                        measured(index)
                    })
                })
                .collect(),
        };
        let medians: ArrayVec<f32, 3> = levels.iter().map(|level| level.median).collect();
        let spread = with_spread.then(|| {
            levels
                .iter()
                .map(|level| level.spread.expect("measured with its spread"))
                .collect()
        });
        let sky = match &mosaic_noise {
            Some(mosaic_noise) => ArrayVec::from(mosaic_noise.sky),
            None => medians.clone(),
        };
        Self {
            medians,
            noise,
            read_share,
            sky,
            spread,
            quantization_sigma: match mosaic_noise {
                Some(mosaic_noise) => mosaic_noise.quantization_sigma,
                None => metadata.quantization_sigma,
            },
            electrons_per_unit: metadata.electrons_per_unit(),
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

/// One slot's robust spread: `1.4826·MAD`, and how many samples it was taken over, which its
/// standard error goes as.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct SlotSpread {
    pub(crate) sigma: f32,
    pub(crate) samples: usize,
}

/// One slot's level: its median, and its robust spread when asked for.
#[derive(Debug, Clone, Copy)]
struct SlotLevel {
    median: f32,
    spread: Option<SlotSpread>,
}

impl SlotLevel {
    /// The median of the samples of `plane`, rows `width` long, whose index and position `keep`
    /// names, and with `with_spread` their `1.4826·MAD`. Both 0 when `keep` names none: a frame
    /// with nothing measured there contributes at none of them, so the figures are never read —
    /// but the median of nothing would panic.
    fn of(
        plane: &[f32],
        width: usize,
        with_spread: bool,
        keep: impl Fn(usize, Vec2us) -> bool,
    ) -> Self {
        let kept = |y: usize, x: usize| keep(y * width + x, Vec2us::new(x, y));
        let mut count = 0;
        for (y, row) in plane.chunks(width).enumerate() {
            count += (0..row.len()).filter(|&x| kept(y, x)).count();
        }
        // Counted first, so the copy is the one its sort needs and no larger.
        let mut samples = Vec::with_capacity(count);
        for (y, row) in plane.chunks(width).enumerate() {
            samples.extend(
                row.iter()
                    .enumerate()
                    .filter(|&(x, _)| kept(y, x))
                    .map(|(_, &sample)| sample),
            );
        }
        if samples.is_empty() {
            return Self {
                median: 0.0,
                spread: with_spread.then_some(SlotSpread {
                    sigma: 0.0,
                    samples: 0,
                }),
            };
        }
        if with_spread {
            let samples_taken = samples.len();
            let stats = MedianMad::of_mut(&mut samples);
            Self {
                median: stats.median,
                spread: Some(SlotSpread {
                    sigma: stats.sigma(),
                    samples: samples_taken,
                }),
            }
        } else {
            Self {
                median: statistics::median_mut(&mut samples),
                spread: None,
            }
        }
    }
}

#[cfg(test)]
mod tests;
