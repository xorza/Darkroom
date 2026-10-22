//! Per-frame robust statistics, with the white noise measured before any interpolation.

use std::sync::Arc;

use arrayvec::ArrayVec;
use imaginarium::Buffer2;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::image::bg_split::BgSplit;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::quality_flags::QualityFlags;
use crate::image::stackable_image::StackableImage;
use crate::math::statistics;
use crate::math::statistics::median_mad::MedianMad;
use crate::math::vec2us::Vec2us;
use crate::noise::ccd_noise::CcdNoise;
use crate::noise::mosaic_noise_measure::MosaicNoiseMeasure;
use crate::noise::mrs_noise::{self, NoisePlane, PixelGains};

/// Per-frame statistics, per slot: a channel, or a color of a mosaic, whose one channel holds three
/// colors of their own level and noise.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FrameStats {
    /// Each slot's median over its pixels the source measured.
    pub(crate) medians: ArrayVec<f32, 3>,
    /// The white-noise variance where no flat amplified it, without the signal MAD includes, split
    /// by how a flat amplified it: per color of a mosaic by [`MosaicNoiseMeasure`], per color of
    /// the mosaic a demosaiced frame came from, or per channel by [`NoisePlane`]. Measured over the
    /// pixels no flag names; unflattened for a frame no flat divided.
    pub(crate) bg: ArrayVec<BgSplit, 3>,
    /// The level `bg` was measured at, in the same slots: each color's median of the
    /// mosaic, or each channel's median.
    pub(crate) sky: ArrayVec<f32, 3>,
    /// Each slot's robust spread over the same pixels as its median: noise and signal together.
    /// Measured only where a normalization scales frames one by one, which a drizzle of mosaics
    /// does ([`Spreads::Measured`]); `None` elsewhere.
    pub(crate) spread: Option<ArrayVec<SlotSpread, 3>>,
    pub(crate) quantization_sigma: Option<f32>,
    /// From [`ImageMetadata::electrons_per_unit`], when the source states its gain.
    ///
    /// [`ImageMetadata::electrons_per_unit`]: crate::image::image_metadata::ImageMetadata::electrons_per_unit
    pub(crate) electrons_per_unit: Option<f32>,
}

impl FrameStats {
    /// Measure each slot's median on `image`, and its white noise before any interpolation: on a
    /// mosaic, per color; on a demosaiced frame, the
    /// [`MosaicNoise`](crate::image::mosaic_noise::MosaicNoise) its demosaic measured, because the
    /// frame's own correlated pixels would understate it; elsewhere, per channel.
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
        Self::measure_as(image, Spreads::Skipped)
    }

    /// The most bytes [`Self::measure`] holds beside a frame of `dimensions` with `threads`
    /// workers: its noise first — a mosaic's by [`MosaicNoiseMeasure`], any other frame's by
    /// [`NoisePlane`] one channel at a time — then the samples its levels sort, a plane's worth at
    /// most: a mosaic's colors at once, which share its one plane, or a channel at a time.
    pub(crate) fn heap_bytes(
        dimensions: ImageDimensions,
        mosaic: Option<&CfaType>,
        threads: usize,
    ) -> usize {
        let size = dimensions.size();
        let noise = match mosaic {
            Some(cfa_type) => MosaicNoiseMeasure::heap_bytes(size, cfa_type),
            None => mrs_noise::heap_bytes(size, threads),
        };
        noise.max(dimensions.pixel_count() * size_of::<f32>())
    }

    /// [`Self::measure`], and with [`Spreads::Measured`] each slot's [`spread`](Self::spread) from
    /// the samples its median already took.
    pub(crate) fn measure_as(image: &impl StackableImage, spreads: Spreads) -> Self {
        let dimensions = image.dimensions();
        let metadata = image.metadata();
        let flags = image.flags();
        let excluded = |idx: usize| flags.is_some_and(|flags| flags.is_flagged(idx));
        let mosaic = image.cfa_type().filter(CfaType::is_mosaic);
        let mosaic_noise = match mosaic {
            Some(cfa_type) => Some(
                MosaicNoiseMeasure {
                    plane: image.channel(0),
                    cfa_type,
                    flags,
                    flat_gain: image.flat_gain().map(Arc::as_ref),
                    quantization_sigma: metadata.derived.quantization_sigma(),
                }
                .measure(),
            ),
            None => metadata.derived.mosaic_noise().inspect(|_| {
                assert_eq!(
                    dimensions.channels(),
                    3,
                    "a frame's mosaic noise names its three channels"
                );
            }),
        };
        let bg: ArrayVec<BgSplit, 3> = match &mosaic_noise {
            Some(mosaic_noise) => ArrayVec::from(mosaic_noise.bg),
            // One channel at a time: each estimate spreads its tiles over the workers already, and
            // the plan charges one estimate's coefficients.
            None => (0..dimensions.channels())
                .map(|channel| {
                    let size = dimensions.size();
                    NoisePlane {
                        samples: image.channel(channel),
                        excluded,
                    }
                    .bg(image.flat_gain().map(|gain| PixelGains {
                        gain: move |idx| {
                            gain.at(
                                channel,
                                (idx % size.width) as f32,
                                (idx / size.width) as f32,
                            )
                        },
                        bins: gain.bins(channel),
                    }))
                })
                .collect(),
        };
        let nulls = flags.filter(|flags| flags.contains(QualityFlags::NO_DATA));
        let measured =
            |idx: usize| nulls.is_none_or(|nulls| !nulls.at(idx).intersects(QualityFlags::NO_DATA));
        let levels: Vec<SlotLevel> = match mosaic {
            Some(cfa_type) => (0..cfa_type.color_count())
                .into_par_iter()
                .map(|color| {
                    SlotLevel::of(image.channel(0), spreads, |idx, pos| {
                        measured(idx) && cfa_type.slot_at(pos) == color
                    })
                })
                .collect(),
            // One channel at a time: each estimate spreads its tiles over the workers already, and
            // the plan charges one estimate's coefficients.
            None => (0..dimensions.channels())
                .map(|channel| {
                    SlotLevel::of(image.channel(channel), spreads, |idx, _| measured(idx))
                })
                .collect(),
        };
        let medians: ArrayVec<f32, 3> = levels.iter().map(|level| level.median).collect();
        let spread = (spreads == Spreads::Measured).then(|| {
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
            bg,
            sky,
            spread,
            quantization_sigma: match mosaic_noise {
                Some(mosaic_noise) => mosaic_noise.quantization_sigma,
                None => metadata.derived.quantization_sigma(),
            },
            electrons_per_unit: metadata.electrons_per_unit(),
        }
    }

    /// The white noise of a slot where no flat amplified it, raised to the quantization σ.
    pub(crate) fn slot_noise(&self, slot: usize) -> f32 {
        self.bg[slot]
            .sigma()
            .max(self.quantization_sigma.unwrap_or(0.0))
    }

    /// The noise model of one slot: a channel, or a color of a mosaic.
    pub(crate) fn ccd_noise(&self, slot: usize) -> CcdNoise {
        let step = self.quantization_sigma.unwrap_or(0.0);
        CcdNoise {
            bg: self.bg[slot],
            quantization_variance: step * step,
            sky: self.sky[slot],
            electrons_per_unit: self.electrons_per_unit,
        }
    }
}

/// Whether a measurement takes each slot's [`SlotSpread`] beside its median.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Spreads {
    Skipped,
    Measured,
}

/// One slot's robust spread: σ from the MAD, and how many samples it was taken over, which its
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
    /// The median of the samples of `plane` whose index and position `keep` names, and with
    /// [`Spreads::Measured`] their σ from the MAD. Both 0 when `keep` names none: a frame with
    /// nothing measured there contributes at none of them, so the figures are never read — but the
    /// median of nothing would panic.
    fn of(plane: &Buffer2<f32>, spreads: Spreads, keep: impl Fn(usize, Vec2us) -> bool) -> Self {
        let width = plane.width();
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
                spread: (spreads == Spreads::Measured).then_some(SlotSpread {
                    sigma: 0.0,
                    samples: 0,
                }),
            };
        }
        if spreads == Spreads::Measured {
            let stats = MedianMad::of_mut(&mut samples);
            Self {
                median: stats.median,
                spread: Some(SlotSpread {
                    sigma: stats.sigma(),
                    samples: stats.count,
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

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
