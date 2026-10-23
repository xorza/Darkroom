//! [`NormFit`]: a set's norms fitted against its reference frame.

use arrayvec::ArrayVec;
use rayon::prelude::*;

use crate::combine::error::StackError;
use crate::combine::normalization::common_domain::{ChannelCoverage, CommonDomain};
use crate::combine::normalization::fit_scratch::FitScratch;
use crate::combine::normalization::per_channel_norms::{PerChannelNorms, SlotEstimate};
use crate::combine::normalization::photometric_gain::{
    MAD_SIGMA_VARIANCE, MEDIAN_ERROR, PairSide, SamplePair,
};
use crate::combine::normalization::slot_pixels::SlotPixels;
use crate::combine::normalization::slot_planes::{PlaneSampling, SlotPlanes};
use crate::combine::normalization::{FrameNorm, Normalization, SlotNorm};
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
    pub(super) normalization: Fitted,
    pub(super) reference: usize,
}

/// A normalization that fits each frame: an affine gain and offset, or a ratio of medians.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Fitted {
    Global,
    Multiplicative,
}

impl Fitted {
    /// `normalization` as one that fits; `None` for [`Normalization::None`], which fits nothing.
    pub(super) const fn of(normalization: Normalization) -> Option<Self> {
        match normalization {
            Normalization::None => None,
            Normalization::Global => Some(Self::Global),
            Normalization::Multiplicative => Some(Self::Multiplicative),
        }
    }
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
                Fitted::Global => self.global(&pixels)?,
                Fitted::Multiplicative => {
                    let medians = match domain {
                        Some(_) => planes.slot_medians(&pixels)?,
                        None => frames
                            .iter()
                            .map(|frame| frame.source_stats.medians.clone())
                            .collect(),
                    };
                    multiplicative_norms(&medians, self.reference)?
                }
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
    /// channels of its colors — measured pair by pair: each frame against the reference over the
    /// pixels both reached in the slot's channel, by the same fit [`Self::global`] makes and the
    /// same ratio [`multiplicative_norms`] takes.
    ///
    /// The pixels every frame reached are no domain for these: each frame's drops reach a color at
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
            .map_init(FitScratch::default, |scratch, pair_idx| {
                let frame_idx = pair_idx / slot_count;
                let slot = pair_idx % slot_count;
                if frame_idx == reference {
                    return Ok(Some(SlotEstimate::EXACT_IDENTITY));
                }
                let FitScratch {
                    median,
                    frame: ours,
                    reference: theirs,
                    work,
                } = scratch;
                let pair = [&frames[frame_idx], &frames[reference]];
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
                let idxs = domain.stratified_idxs(cancel)?;
                let sampling = PlaneSampling {
                    domain: Some(&domain),
                    idxs: &idxs,
                };
                let mut measured_median = |idx: usize| -> Result<f32, StackError> {
                    Ok(sampling
                        .median(planes.values(idx, slot), median, cancel)?
                        .expect("a pair's medians are measured over its domain"))
                };
                let (frame_median, reference_median) =
                    (measured_median(frame_idx)?, measured_median(reference)?);
                let (frame_variance, reference_variance) = (
                    planes.noise_variance(frame_idx, slot, &idxs)?,
                    planes.noise_variance(reference, slot, &idxs)?,
                );
                match self.normalization {
                    Fitted::Global => {
                        sampling.samples(planes.values(frame_idx, slot), ours, cancel)?;
                        sampling.samples(planes.values(reference, slot), theirs, cancel)?;
                        let pair = SamplePair {
                            frame: PairSide::new(ours, frame_variance, work, cancel)?,
                            reference: PairSide::new(theirs, reference_variance, work, cancel)?,
                        };
                        let paired = pair.photometric_gain(work, cancel)?;
                        Ok(Some(SlotEstimate {
                            norm: SlotNorm {
                                gain: paired.gain,
                                offset: reference_median - frame_median * paired.gain,
                            },
                            relative_error: paired.relative_error,
                        }))
                    }
                    Fitted::Multiplicative => {
                        let norm = SlotNorm::median_ratio(
                            (frame_idx, frame_median),
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
                                / (idxs.len() as f64).sqrt(),
                        }))
                    }
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
        let spread = |idx: usize, slot: usize| {
            frames[idx]
                .source_stats
                .spread
                .as_ref()
                .map(|spread| spread[slot])
        };
        let scale = |idx: usize, slot: usize| -> Result<MeasuredScale, StackError> {
            let stats = &frames[idx].source_stats;
            let spread = spread(idx, slot)
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
                    idx,
                    slot,
                    spread: spread.sigma,
                    noise,
                })
            }
        };
        let median_error = |idx: usize, slot: usize| {
            spread(idx, slot).map_or(f64::INFINITY, |spread| {
                MEDIAN_ERROR * f64::from(spread.sigma)
                    / (spread.samples.max(1) as f64).sqrt()
                    / f64::from(medians[idx][slot])
            })
        };
        frames
            .iter()
            .enumerate()
            .map(|(idx, frame)| {
                (0..frame.source_stats.medians.len())
                    .map(|slot| {
                        if idx == reference {
                            return Ok(SlotEstimate::EXACT_IDENTITY);
                        }
                        match self.normalization {
                            Fitted::Global => {
                                let (ours, theirs) = (scale(idx, slot)?, scale(reference, slot)?);
                                let gain = (theirs.scale / ours.scale) as f32;
                                Ok(SlotEstimate {
                                    norm: SlotNorm {
                                        gain,
                                        offset: medians[reference][slot]
                                            - medians[idx][slot] * gain,
                                    },
                                    relative_error: ours
                                        .relative_error
                                        .hypot(theirs.relative_error),
                                })
                            }
                            Fitted::Multiplicative => Ok(SlotEstimate {
                                norm: SlotNorm::median_ratio(
                                    (idx, medians[idx][slot]),
                                    (reference, medians[reference][slot]),
                                    slot,
                                )?,
                                relative_error: median_error(idx, slot)
                                    .hypot(median_error(reference, slot)),
                            }),
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
        let idxs = pixels
            .iter()
            .map(|pixels| pixels.idxs(cancel))
            .collect::<Result<ArrayVec<_, 3>, StackError>>()?;
        let sampled = SlotSamples {
            planes,
            pixels,
            idxs: &idxs,
        };
        let reference_slots = (0..slot_count)
            .into_par_iter()
            .map(|slot| {
                let mut scratch = FitScratch::default();
                let median = sampled.median(reference, slot, &mut scratch.median)?;
                let samples = if let Some(stored) = sampled.stored(reference, slot) {
                    stored.to_vec()
                } else {
                    sampled.gather(reference, slot, &mut scratch.reference)?;
                    scratch.reference
                };
                let side = PairSide::new(
                    &samples,
                    planes.noise_variance(reference, slot, &idxs[slot])?,
                    &mut scratch.work,
                    cancel,
                )?;
                Ok(ReferenceSlot {
                    median,
                    stats: side.stats,
                    noise_variance: side.noise_variance,
                    samples,
                })
            })
            .collect::<Result<Vec<_>, StackError>>()?;

        let fitted = (0..frames.len() * slot_count)
            .into_par_iter()
            .map_init(FitScratch::default, |scratch, pair_idx| {
                let frame_idx = pair_idx / slot_count;
                let slot = pair_idx % slot_count;
                if frame_idx == reference {
                    return Ok(SlotNorm::IDENTITY);
                }
                let FitScratch {
                    median,
                    frame,
                    work,
                    ..
                } = scratch;
                let median = sampled.median(frame_idx, slot, median)?;
                let samples = if let Some(stored) = sampled.stored(frame_idx, slot) {
                    stored
                } else {
                    sampled.gather(frame_idx, slot, frame)?;
                    frame
                };
                let reference = &reference_slots[slot];
                let pair = SamplePair {
                    frame: PairSide::new(
                        samples,
                        planes.noise_variance(frame_idx, slot, &idxs[slot])?,
                        work,
                        cancel,
                    )?,
                    reference: reference.side(),
                };
                let gain = pair.photometric_gain(work, cancel)?.gain;
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

/// Where the global fit reads a frame's slot: its own samples when it stores them and every frame
/// covers every pixel, its values at the slot's `idxs` otherwise.
#[derive(Debug, Clone, Copy)]
struct SlotSamples<'a> {
    planes: SlotPlanes<'a>,
    pixels: &'a [SlotPixels<'a>],
    idxs: &'a [Vec<usize>],
}

impl<'a> SlotSamples<'a> {
    /// Frame `frame`'s median of `slot`: over the pixels the frames share, or the source's.
    fn median(
        &self,
        frame: usize,
        slot: usize,
        median: &mut RadixMedian,
    ) -> Result<f32, StackError> {
        let shared = self.sampling(slot).median(
            self.planes.values(frame, slot),
            median,
            self.planes.cancel,
        )?;
        Ok(shared.unwrap_or(self.planes.frames[frame].source_stats.medians[slot]))
    }

    /// Frame `frame`'s stored samples of `slot`, when it stores them over every pixel.
    fn stored(&self, frame: usize, slot: usize) -> Option<&'a [f32]> {
        let SlotPixels::Everywhere(_) = &self.pixels[slot] else {
            return None;
        };
        let samples = &self.planes.frames[frame].samples.as_ref()?[slot];
        debug_assert_eq!(samples.samples(), self.idxs[slot].len());
        Some(samples.chunk(0..samples.samples()))
    }

    /// Frame `frame`'s values of `slot` at its indices, into `samples`.
    fn gather(&self, frame: usize, slot: usize, samples: &mut Vec<f32>) -> Result<(), StackError> {
        self.sampling(slot)
            .samples(self.planes.values(frame, slot), samples, self.planes.cancel)
    }

    fn sampling(&self, slot: usize) -> PlaneSampling<'a> {
        PlaneSampling {
            domain: self.pixels[slot].shared(),
            idxs: &self.idxs[slot],
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
/// [`MAX_STATISTIC_SAMPLES`] per color, taken here as the smaller of that and the spread's count.
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
        .map(|(idx, frame)| {
            let slots = frame
                .iter()
                .zip(&medians[reference])
                .enumerate()
                .map(|(slot, (&median, &reference_median))| {
                    SlotNorm::median_ratio((idx, median), (reference, reference_median), slot)
                })
                .collect::<Result<_, _>>()?;
            Ok(FrameNorm { slots })
        })
        .collect()
}
