//! [`NormFit`]: a set's norms fitted against its reference frame.

use arrayvec::ArrayVec;
use rayon::prelude::*;

use crate::combine::error::StackError;
use crate::combine::normalization::common_domain::{ChannelCoverage, CommonDomain};
use crate::combine::normalization::per_channel_norms::{PerChannelNorms, SlotEstimate};
use crate::combine::normalization::photometric_gain::{
    MAD_SIGMA_VARIANCE, MEDIAN_ERROR, PairSide, SamplePair,
};
use crate::combine::normalization::slot_pixels::SlotPixels;
use crate::combine::normalization::slot_planes::{PlaneMeasurement, PlaneSampling, SlotPlanes};
use crate::combine::normalization::{FrameNorm, SlotNorm};
use crate::combine::stack_config::Normalization;
use crate::frame::frame_stats::SlotSpread;
use crate::frame_store::stored_frame::StoredFrame;
use crate::math::statistics::median_mad::MedianMad;
use crate::math::statistics::radix_median::RadixMedian;
use crate::math::statistics::subsample::MAX_STATISTIC_SAMPLES;

/// A fitted normalization of a set: its frames, how they are fitted, and the frame they are fitted
/// against.
#[derive(Debug, Clone, Copy)]
pub(super) struct NormFit<'a> {
    pub(super) planes: SlotPlanes<'a>,
    pub(super) normalization: Normalization,
    pub(super) reference: usize,
}

/// Which estimator gave a set's fitted norms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Estimator {
    /// Over the pixels every frame covers, or over every pixel.
    Common,
    /// Each frame against the reference over the pixels the two share.
    Pairwise,
    /// Each frame by the statistics its source was measured with.
    PerFrame,
}

impl NormFit<'_> {
    /// The fitted norms, measured where the frames allow: over every pixel when each frame covers
    /// all of them, where the statistics measured on the sources at load already describe the
    /// pixels combined; over the common domain otherwise, since each source was measured over a
    /// different set of pixels than the one the frames share; and pair by pair when a frame is
    /// gathered in each channel by its own rule — see [`Self::pairwise`] — or each frame by its own
    /// statistics — see [`Self::per_frame`] — whichever [`PerChannelNorms::choose`] finds the more
    /// precise. One rule holds for the whole set, so no two frames are scaled by different
    /// estimators.
    pub(super) fn frame_norms(self) -> Result<Vec<FrameNorm>, StackError> {
        let planes = self.planes;
        let frames = planes.frames;
        let gated = frames.iter().any(|frame| !frame.quality.is_none());
        let per_channel = frames.iter().any(|frame| frame.quality.per_channel());
        let mut estimator = Estimator::Common;
        let norms = if per_channel {
            let chosen = PerChannelNorms::choose(self.pairwise()?, self.per_frame())?;
            estimator = chosen.estimator;
            chosen.norms
        } else {
            let all: Vec<&StoredFrame> = frames.iter().collect();
            let domain = gated
                .then(|| {
                    CommonDomain::build(
                        ChannelCoverage {
                            frames: &all,
                            channel: 0,
                            pixel_count: planes.dimensions.pixel_count(),
                        },
                        planes.cancel,
                    )
                })
                .transpose()?;
            let pixels = SlotPixels::of_slots(planes, domain.as_ref())?;
            match self.normalization {
                Normalization::Global => self.global(&pixels)?,
                Normalization::Multiplicative => {
                    let medians = match domain {
                        Some(_) => planes.slot_medians(&pixels)?,
                        None => frames
                            .iter()
                            .map(|frame| frame.source_stats.medians.clone())
                            .collect(),
                    };
                    multiplicative_norms(&medians, self.reference)?
                }
                Normalization::None => unreachable!("handled by the caller"),
            }
        };
        tracing::info!(
            frame_count = frames.len(),
            slots = planes.slots.count(),
            ref_frame = self.reference,
            common_domain = gated && !per_channel,
            ?estimator,
            normalization = ?self.normalization,
            "Computed normalization"
        );
        Ok(norms)
    }

    /// The norms of frames gathered in each channel by its own rule — a mosaic drizzled into the
    /// channels of its colours — measured pair by pair: each frame against the reference over the
    /// pixels both reached in the slot's channel, by the same fit [`Self::global`] makes and the
    /// same ratio [`multiplicative_norms`] takes.
    ///
    /// The pixels every frame reached are no domain for these: each frame's drops reach a colour at
    /// only part of the grid — at scale 2 and pixfrac 1, a Bayer red at 9 output pixels of 16 — so
    /// their intersection thins out with every frame added. A pair keeps as many as two frames
    /// share, and a pair is all either fit compares. Above scale 1 a pair can still share none: two
    /// frames a photosite apart put one's red where the other's green fell. The pair's domain is
    /// then empty, and its estimate is `None` rather than a comparison over nothing.
    ///
    /// Each estimate carries its gain's relative standard error: the Deming fit's for `Global`
    /// ([`SamplePair::photometric_gain`]), and for `Multiplicative` the two medians' from the
    /// measurement noise alone, `√(π/2)·σ/√n` each, since both frames are read at the same pixels
    /// and their structure cancels in the ratio.
    fn pairwise(self) -> Result<Vec<ArrayVec<Option<SlotEstimate>, 3>>, StackError> {
        let planes = self.planes;
        let frames = planes.frames;
        let cancel = planes.cancel;
        let reference = self.reference;
        let slot_count = planes.slots.count();
        let fitted = (0..frames.len() * slot_count)
            .into_par_iter()
            .map_init(RadixMedian::default, |median, pair_index| {
                let frame_index = pair_index / slot_count;
                let slot = pair_index % slot_count;
                if frame_index == reference {
                    return Ok(Some(SlotEstimate::EXACT_IDENTITY));
                }
                let pair = [&frames[frame_index], &frames[reference]];
                let coverage = ChannelCoverage {
                    frames: &pair,
                    channel: planes.slots.channel(slot),
                    pixel_count: planes.dimensions.pixel_count(),
                };
                let domain = match CommonDomain::build_in_job(coverage, cancel) {
                    Ok(domain) => domain,
                    Err(StackError::NoCommonCoverage) => return Ok(None),
                    Err(error) => return Err(error),
                };
                let indices = domain.stratified_indices(cancel)?;
                let sampling = PlaneSampling {
                    domain: Some(&domain),
                    indices: &indices,
                };
                let [ours, theirs] = [frame_index, reference]
                    .map(|index| sampling.measure(planes.values(index, slot), median, cancel));
                let (ours, theirs) = (ours?, theirs?);
                let measured_median = |measured: &PlaneMeasurement| {
                    measured
                        .median
                        .expect("a pair's medians are measured over its domain")
                };
                let (frame_median, reference_median) =
                    (measured_median(&ours), measured_median(&theirs));
                let (frame_variance, reference_variance) = (
                    planes.noise_variance(frame_index, slot, &indices)?,
                    planes.noise_variance(reference, slot, &indices)?,
                );
                match self.normalization {
                    Normalization::Global => {
                        let pair = SamplePair {
                            frame: PairSide::new(&ours.samples, frame_variance, cancel)?,
                            reference: PairSide::new(&theirs.samples, reference_variance, cancel)?,
                        };
                        let paired = pair.photometric_gain(cancel)?;
                        Ok(Some(SlotEstimate {
                            norm: SlotNorm {
                                gain: paired.gain,
                                offset: reference_median - frame_median * paired.gain,
                            },
                            relative_error: paired.relative_error,
                        }))
                    }
                    Normalization::Multiplicative => {
                        let norm = SlotNorm::median_ratio(
                            (frame_index, frame_median),
                            (reference, reference_median),
                            slot,
                        )?;
                        let relative =
                            |variance: f64, median: f32| variance / f64::from(median).powi(2);
                        Ok(Some(SlotEstimate {
                            norm,
                            relative_error: MEDIAN_ERROR
                                * (relative(frame_variance, frame_median)
                                    + relative(reference_variance, reference_median))
                                .sqrt()
                                / (indices.len() as f64).sqrt(),
                        }))
                    }
                    Normalization::None => unreachable!("handled by the caller"),
                }
            })
            .collect::<Result<Vec<_>, StackError>>()?;
        Ok(fitted
            .chunks(slot_count)
            .map(|slots| slots.iter().copied().collect())
            .collect())
    }

    /// The norms of frames gathered in each channel by their own rule, measured one by one from the
    /// statistics each source was measured with, as Siril's default normalization and PixInsight's
    /// measure each frame: no pixel is paired, so no pair of frames has to share one.
    ///
    /// A slot's location is its source median. Its scale is its robust spread with the white noise
    /// taken out, `√(spread² − σ²)`: the signal's own spread, which a frame's transparency scales,
    /// where the spread alone would read a noisier frame as a brighter one. Subtracting variances
    /// is exact for independent Gaussian parts and an approximation otherwise; `σ` is the noise
    /// where no flat amplified it, so on a vignetted frame the noise left in is a little more than
    /// none, alike in every frame of one setup. `Global` takes the gain as the reference's scale
    /// over the frame's and puts the medians on each other; `Multiplicative` takes the ratio of the
    /// medians.
    ///
    /// A slot whose `spread² − σ²` does not clear [`SIGNAL_SIGNIFICANCE`] standard errors of its
    /// own estimate holds no signal to scale by: on pure noise the difference scatters about zero,
    /// and its square root would be a gain drawn from that scatter.
    ///
    /// Each estimate carries its gain's relative standard error, the reference's and the frame's
    /// added in quadrature: for `Global` each scale's, half the relative error of `spread² − σ²`;
    /// for `Multiplicative` each median's, `√(π/2)·spread/√n` — the spread, not the noise, since
    /// two frames measured over different pixels differ by their structure too. A frame measured
    /// without its spread has no error to state, an infinite one.
    ///
    /// # Errors
    /// [`StackError::NoSignalScale`] for a `Global` slot without a significant signal spread, and
    /// as [`SlotNorm::median_ratio`] for `Multiplicative`.
    fn per_frame(self) -> Result<Vec<ArrayVec<SlotEstimate, 3>>, StackError> {
        let frames = self.planes.frames;
        let reference = self.reference;
        let medians: Vec<ArrayVec<f32, 3>> = frames
            .iter()
            .map(|frame| frame.source_stats.medians.clone())
            .collect();
        let spread = |index: usize, slot: usize| {
            frames[index]
                .source_stats
                .spread
                .as_ref()
                .map(|spread| spread[slot])
        };
        let scale = |index: usize, slot: usize| -> Result<MeasuredScale, StackError> {
            let stats = &frames[index].source_stats;
            let spread = spread(index, slot)
                .expect("a frame gathered per channel was measured with its spread");
            let noise = stats.slot_noise(slot);
            let signal = f64::from(spread.sigma).powi(2) - f64::from(noise).powi(2);
            if signal > SIGNAL_SIGNIFICANCE * spread_variance_error(spread, noise) {
                Ok(MeasuredScale {
                    scale: signal.sqrt(),
                    relative_error: signal_variance_error(spread, noise) / (2.0 * signal),
                })
            } else {
                Err(StackError::NoSignalScale {
                    index,
                    slot,
                    spread: spread.sigma,
                    noise,
                })
            }
        };
        let median_error = |index: usize, slot: usize| {
            spread(index, slot).map_or(f64::INFINITY, |spread| {
                MEDIAN_ERROR * f64::from(spread.sigma)
                    / (spread.samples.max(1) as f64).sqrt()
                    / f64::from(medians[index][slot])
            })
        };
        frames
            .iter()
            .enumerate()
            .map(|(index, frame)| {
                (0..frame.source_stats.medians.len())
                    .map(|slot| {
                        if index == reference {
                            return Ok(SlotEstimate::EXACT_IDENTITY);
                        }
                        match self.normalization {
                            Normalization::Global => {
                                let (ours, theirs) = (scale(index, slot)?, scale(reference, slot)?);
                                let gain = (theirs.scale / ours.scale) as f32;
                                Ok(SlotEstimate {
                                    norm: SlotNorm {
                                        gain,
                                        offset: medians[reference][slot]
                                            - medians[index][slot] * gain,
                                    },
                                    relative_error: ours
                                        .relative_error
                                        .hypot(theirs.relative_error),
                                })
                            }
                            Normalization::Multiplicative => Ok(SlotEstimate {
                                norm: SlotNorm::median_ratio(
                                    (index, medians[index][slot]),
                                    (reference, medians[reference][slot]),
                                    slot,
                                )?,
                                relative_error: median_error(index, slot)
                                    .hypot(median_error(reference, slot)),
                            }),
                            Normalization::None => unreachable!("handled by the caller"),
                        }
                    })
                    .collect()
            })
            .collect()
    }

    /// Fit every frame's gain and offset against the reference frame directly, slot by slot, over
    /// `pixels`.
    ///
    /// Each non-reference frame is paired with the reference through
    /// [`SamplePair::photometric_gain`]'s errors-in-variables fit, on a stratified sample of the
    /// slot's measured pixels; the offset then puts the frame's median on the reference's. The
    /// reference itself is the identity by definition.
    ///
    /// Fitting against the reference rather than against frame 0 and rescaling afterwards matters:
    /// the fit clips residuals and weights each side by its own noise, so `gain(a→c)` is not
    /// `gain(a→b) / gain(c→b)` — chaining through an arbitrary frame would put its noise into every
    /// other frame's scale.
    fn global(self, pixels: &[SlotPixels<'_>]) -> Result<Vec<FrameNorm>, StackError> {
        let planes = self.planes;
        let frames = planes.frames;
        let cancel = planes.cancel;
        let reference = self.reference;
        let slot_count = pixels.len();
        let indices = pixels
            .iter()
            .map(|pixels| pixels.indices(cancel))
            .collect::<Result<ArrayVec<_, 3>, StackError>>()?;
        let measure = |frame: usize, slot: usize, median: &mut RadixMedian| {
            if let (SlotPixels::Everywhere(_), Some(samples)) =
                (&pixels[slot], &frames[frame].samples)
            {
                let samples = &samples[slot];
                debug_assert_eq!(samples.samples(), indices[slot].len());
                return Ok(PlaneMeasurement {
                    median: None,
                    samples: samples.chunk(0..samples.samples()).to_vec(),
                });
            }
            PlaneSampling {
                domain: pixels[slot].shared(),
                indices: &indices[slot],
            }
            .measure(planes.values(frame, slot), median, cancel)
        };
        let reference_slots = (0..slot_count)
            .into_par_iter()
            .map(|slot| {
                let frame = &frames[reference];
                let measured = measure(reference, slot, &mut RadixMedian::default())?;
                let side = PairSide::new(
                    &measured.samples,
                    planes.noise_variance(reference, slot, &indices[slot])?,
                    cancel,
                )?;
                Ok(ReferenceSlot {
                    median: measured.median.unwrap_or(frame.source_stats.medians[slot]),
                    stats: side.stats,
                    noise_variance: side.noise_variance,
                    samples: measured.samples,
                })
            })
            .collect::<Result<Vec<_>, StackError>>()?;

        let fitted = (0..frames.len() * slot_count)
            .into_par_iter()
            .map_init(RadixMedian::default, |median, pair_index| {
                let frame_index = pair_index / slot_count;
                let slot = pair_index % slot_count;
                if frame_index == reference {
                    return Ok(SlotNorm::IDENTITY);
                }
                let frame = &frames[frame_index];
                let measured = measure(frame_index, slot, median)?;
                let median = measured.median.unwrap_or(frame.source_stats.medians[slot]);
                let reference = &reference_slots[slot];
                let pair = SamplePair {
                    frame: PairSide::new(
                        &measured.samples,
                        planes.noise_variance(frame_index, slot, &indices[slot])?,
                        cancel,
                    )?,
                    reference: reference.side(),
                };
                let gain = pair.photometric_gain(cancel)?.gain;
                Ok(SlotNorm {
                    gain,
                    offset: reference.median - median * gain,
                })
            })
            .collect::<Result<Vec<_>, StackError>>()?;

        Ok(fitted
            .chunks(slot_count)
            .map(|slots| FrameNorm {
                slots: slots.iter().copied().collect(),
            })
            .collect())
    }
}

/// One slot of the reference frame, measured once and paired against every other frame.
#[derive(Debug)]
struct ReferenceSlot {
    median: f32,
    samples: Vec<f32>,
    stats: MedianMad,
    noise_variance: f64,
}

impl ReferenceSlot {
    /// The reference's side of a pair.
    const fn side(&self) -> PairSide<'_> {
        PairSide {
            samples: self.samples.as_slice(),
            stats: self.stats,
            noise_variance: self.noise_variance,
        }
    }
}

/// A slot's signal scale, `√(spread² − σ²)`, and its relative standard error.
#[derive(Debug, Clone, Copy)]
struct MeasuredScale {
    scale: f64,
    relative_error: f64,
}

/// Standard errors of its own estimate that a slot's `spread² − σ²` has to clear to count as
/// signal: at 3, pure Gaussian noise passes once in about 740 slots, and that one is still refused
/// loud enough to name, as no other wrong gain would be.
const SIGNAL_SIGNIFICANCE: f64 = 3.0;

/// The standard error of `spread² − σ²` on pure Gaussian noise of σ `noise`: each side a
/// `1.4826·MAD` estimate of σ, whose variance is `σ²/(2n)` over its 36.75% efficiency against the
/// standard deviation, `1.3605·σ²/n`, so `5.442·σ⁴/n` for its square. The spread takes `n` from its
/// own count; the white noise from pairs of photosites on rows thinned to
/// [`MAX_STATISTIC_SAMPLES`] per colour, taken here as the smaller of that and the spread's count.
fn spread_variance_error(spread: SlotSpread, noise: f32) -> f64 {
    squared_spreads_error(noise, spread, noise)
}

/// The standard error of `spread² − σ²` where the spread holds signal as well: each square's
/// relative variance `4 · 1.3605/n` at its own size, `σ_spread⁴` and `σ⁴`, the counts as
/// [`spread_variance_error`] takes them.
fn signal_variance_error(spread: SlotSpread, noise: f32) -> f64 {
    squared_spreads_error(spread.sigma, spread, noise)
}

/// The standard error of `spread² − noise²` for a spread of size `spread_sigma` over
/// `spread.samples` and a noise over the thinned pairs.
fn squared_spreads_error(spread_sigma: f32, spread: SlotSpread, noise: f32) -> f64 {
    let spread_samples = spread.samples.max(1) as f64;
    let noise_samples = spread.samples.clamp(1, MAX_STATISTIC_SAMPLES) as f64;
    (4.0 * MAD_SIGMA_VARIANCE
        * (f64::from(spread_sigma).powi(4) / spread_samples
            + f64::from(noise).powi(4) / noise_samples))
        .sqrt()
}

/// `gain = median_ref / median`, per slot.
///
/// # Errors
/// As [`SlotNorm::median_ratio`].
fn multiplicative_norms(
    medians: &[ArrayVec<f32, 3>],
    reference: usize,
) -> Result<Vec<FrameNorm>, StackError> {
    medians
        .iter()
        .enumerate()
        .map(|(index, frame)| {
            let slots = frame
                .iter()
                .zip(&medians[reference])
                .enumerate()
                .map(|(slot, (&median, &reference_median))| {
                    SlotNorm::median_ratio((index, median), (reference, reference_median), slot)
                })
                .collect::<Result<_, _>>()?;
            Ok(FrameNorm { slots })
        })
        .collect()
}
