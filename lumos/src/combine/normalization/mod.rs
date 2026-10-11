//! Putting every frame on one photometric scale before they are combined.
//!
//! Frames of the same field differ in sky level and transparency, so combining them raw would let
//! the brightest dominate and turn rejection into a vote about exposure rather than about
//! outliers. Each frame gets an affine `gain`/`offset` per slot — a channel, or a colour of a
//! mosaic, whose colours drift apart over a session of twilight flats — measured against a
//! reference, the least noisy of the set, and the combine applies it as it gathers.
//!
//! `Normalization::Global` fits every frame's gain against the reference by
//! [`photometric_gain`]'s errors-in-variables fit over paired pixels, and its offset from the two
//! medians. `Normalization::Multiplicative` takes the ratio of medians. A slot is measured over
//! its own pixels — a colour over its photosites — and coverage narrows them: frames that each
//! cover every pixel are measured over all of them, and when any frame contributes at only some
//! pixels — a warp's edge, or a source's declared nulls — over [`common_domain`], the pixels every
//! frame reached. Frames gathered in each channel by their own rule, a drizzle of mosaics, are
//! measured pair by pair against the reference, or one by one when some pair shares no pixel —
//! see [`fitted_frame_norms`].

pub(crate) mod common_domain;
pub(crate) mod photometric_gain;

use arrayvec::ArrayVec;
use common::CancelToken;
use rayon::prelude::*;

use crate::combine::cache::slots::Slots;
use crate::combine::config::Normalization;
use crate::combine::error::StackError;
use crate::combine::normalization::common_domain::{CommonDomain, WORD_BITS};
use crate::combine::normalization::photometric_gain::{
    MAD_SIGMA_VARIANCE, MEDIAN_ERROR, paired_photometric_gain, sample_stats,
};
use crate::frame_store::frame_stats::SlotSpread;
use crate::frame_store::stored_frame::StoredFrame;
use crate::frame_store::stored_plane::StoredPlane;
use crate::frame_store::stratified_samples::{SAMPLE_LIMIT, StratifiedSamples};
use crate::io::cancelled::Cancelled;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::sample_domain::DomainMap;
use crate::math::statistics::MedianMad;
use crate::math::statistics::radix_median::RadixMedian;
use crate::math::statistics::subsample::MAX_STATISTIC_SAMPLES;

/// One slot's affine normalization, applied as `normalized = raw * gain + offset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SlotNorm {
    pub(crate) gain: f32,
    pub(crate) offset: f32,
}

impl SlotNorm {
    const IDENTITY: Self = Self {
        gain: 1.0,
        offset: 0.0,
    };

    /// `gain = median_ref / median` for slot `slot` of the frame of `(index, median)` against the
    /// reference's `(index, median)`.
    ///
    /// # Errors
    /// [`StackError::NonPositiveMedian`] when a median is not positive: a ratio to it scales
    /// nothing, and unit gain in its place would combine the frame at a scale no one measured.
    fn median_ratio(
        (index, median): (usize, f32),
        (reference, reference_median): (usize, f32),
        slot: usize,
    ) -> Result<Self, StackError> {
        if median > 0.0 && reference_median > 0.0 {
            Ok(Self {
                gain: reference_median / median,
                offset: 0.0,
            })
        } else {
            let (index, median) = if median > 0.0 {
                (reference, reference_median)
            } else {
                (index, median)
            };
            Err(StackError::NonPositiveMedian {
                index,
                slot,
                median,
            })
        }
    }
}

/// A frame's affine normalization, one per [`Slots`] slot.
#[derive(Debug, Clone)]
pub(crate) struct FrameNorm {
    pub(crate) slots: ArrayVec<SlotNorm, 3>,
}

impl FrameNorm {
    /// The per-frame affine every frame is combined through, or `None` when every frame is taken as
    /// it stands.
    ///
    /// Expressed in the domain of the first frame that declares one — the domain the stacked
    /// product records. With no normalization each frame is only converted into it (gain `scale_i /
    /// scale_ref`, offset from the pedestals); a fitted normalization maps every frame onto its
    /// reference frame, so its gains and offsets are then converted from the reference frame's
    /// domain the same way. Frames whose domains agree get exactly the norms they had before
    /// conversion existed.
    pub(crate) fn measure(
        frames: &[StoredFrame],
        dimensions: ImageDimensions,
        slots: Slots,
        normalization: Normalization,
        cancel: &CancelToken,
    ) -> Result<Option<Vec<Self>>, StackError> {
        let to_domain = domain_maps(frames);
        if normalization == Normalization::None {
            return Ok(to_domain
                .iter()
                .any(|&map| map != DomainMap::IDENTITY)
                .then(|| {
                    to_domain
                        .iter()
                        .map(|map| FrameNorm {
                            slots: (0..slots.count())
                                .map(|_| SlotNorm {
                                    gain: map.gain as f32,
                                    offset: map.offset as f32,
                                })
                                .collect(),
                        })
                        .collect()
                }));
        }
        Cancelled::check(cancel)?;
        let reference = select_reference_frame(frames, &to_domain);
        let mut norms =
            fitted_frame_norms(frames, dimensions, slots, normalization, reference, cancel)?;
        // A fitted norm lands each frame on the reference frame's raw values; the reference's own
        // map then carries those into the shared domain: `map(gain·x + offset)`.
        let map = to_domain[reference];
        if map != DomainMap::IDENTITY {
            for slot in norms.iter_mut().flat_map(|norm| norm.slots.iter_mut()) {
                slot.gain = (map.gain * f64::from(slot.gain)) as f32;
                slot.offset = (map.gain * f64::from(slot.offset) + map.offset) as f32;
            }
        }
        Ok(Some(norms))
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

/// What [`measure_plane`] measures: a slot's median over the pixels the frames share when they
/// do not all cover every pixel, and its values at the stratified sample indices.
#[derive(Debug)]
struct PlaneMeasurement {
    median: Option<f32>,
    samples: Vec<f32>,
}

const _: () = assert!(
    Cancelled::POLL_CHUNK.is_multiple_of(WORD_BITS),
    "a gather chunk starts on a mask word"
);

/// The pixels one slot is measured over.
#[derive(Debug)]
enum SlotPixels<'a> {
    /// Every pixel of the slot — a channel's, or a colour's photosites — each frame covering all of
    /// them, sampled the way a frame on disk sampled itself.
    Everywhere(StratifiedSamples),
    /// The pixels every frame covers.
    Common(&'a CommonDomain),
    /// The photosites of one colour among the pixels every frame covers.
    CommonColour(CommonDomain),
}

impl<'a> SlotPixels<'a> {
    /// Each slot's pixels: its channel's, or its colour's photosites, among the pixels every frame
    /// covers when `domain` holds them.
    ///
    /// # Errors
    /// [`StackError::NoCommonCoverage`] when a slot has no pixel to measure.
    fn of_slots(
        slots: Slots,
        domain: Option<&'a CommonDomain>,
        dimensions: ImageDimensions,
        cancel: &CancelToken,
    ) -> Result<ArrayVec<Self, 3>, StackError> {
        let mosaic = slots.mosaic();
        (0..slots.count())
            .map(|slot| match (domain, &mosaic) {
                (None, mosaic) => {
                    let samples = StratifiedSamples::new(dimensions.size(), mosaic.as_ref(), slot);
                    if samples.len() == 0 {
                        return Err(StackError::NoCommonCoverage);
                    }
                    Ok(Self::Everywhere(samples))
                }
                (Some(domain), None) => Ok(Self::Common(domain)),
                (Some(domain), Some(cfa_type)) => Ok(Self::CommonColour(CommonDomain::of_colour(
                    domain,
                    dimensions.size(),
                    cfa_type,
                    slot,
                    cancel,
                )?)),
            })
            .collect()
    }

    /// The pixels a median is measured over again, when the frames do not all cover every pixel;
    /// `None` when the statistics measured on each source at load describe the pixels combined.
    const fn shared(&self) -> Option<&CommonDomain> {
        match self {
            Self::Everywhere(_) => None,
            Self::CommonColour(domain) => Some(domain),
            Self::Common(domain) => Some(domain),
        }
    }

    /// The sampled pixel indices, ascending.
    fn indices(&self, cancel: &CancelToken) -> Result<Vec<usize>, StackError> {
        match self {
            Self::Everywhere(samples) => Ok(samples.indices().collect()),
            Self::Common(domain) => stratified_indices(domain, cancel),
            Self::CommonColour(domain) => stratified_indices(domain, cancel),
        }
    }
}

/// The map that expresses each frame in the domain of the first frame declaring one; the identity
/// for a frame that declares none, or when none does.
///
/// The combine's constructors admit the set's facts first ([`SetFacts`]), so every declared domain
/// converts.
///
/// [`SetFacts`]: crate::ingest::set_facts::SetFacts
fn domain_maps(frames: &[StoredFrame]) -> Vec<DomainMap> {
    let reference = frames
        .iter()
        .find_map(|frame| frame.source_stats.facts.domain.as_ref());
    frames
        .iter()
        .map(
            |frame| match (&frame.source_stats.facts.domain, reference) {
                (Some(domain), Some(reference)) => domain
                    .conversion_to(reference)
                    .expect("the frame set's sample domains were validated as convertible"),
                _ => DomainMap::IDENTITY,
            },
        )
        .collect()
}

/// The fitted norms, measured where the frames allow: over every pixel when each frame covers
/// all of them, where the statistics measured on the sources at load already describe the
/// pixels combined; over the common domain otherwise, since each source was measured over a
/// different set of pixels than the one the frames share; and pair by pair when a frame is
/// gathered in each channel by its own rule — see [`pairwise_norms`] — or each frame by its own
/// statistics — see [`per_frame_norms`] — whichever [`PerChannelNorms::choose`] finds the more
/// precise. One rule holds for the whole set, so no two frames are scaled by different estimators.
fn fitted_frame_norms(
    frames: &[StoredFrame],
    dimensions: ImageDimensions,
    slots: Slots,
    normalization: Normalization,
    reference: usize,
    cancel: &CancelToken,
) -> Result<Vec<FrameNorm>, StackError> {
    let pixel_count = dimensions.pixel_count();
    let planes = SlotPlanes {
        frames,
        slots,
        pixel_count,
    };
    let gated = frames.iter().any(|frame| !frame.quality.is_none());
    let per_channel = frames.iter().any(|frame| frame.quality.per_channel());
    let mut estimator = "common";
    let norms = if per_channel {
        let chosen = PerChannelNorms::choose(
            pairwise_norms(planes, normalization, reference, cancel)?,
            per_frame_norms(frames, normalization, reference),
        )?;
        estimator = chosen.estimator;
        chosen.norms
    } else {
        let all: Vec<&StoredFrame> = frames.iter().collect();
        let domain = gated
            .then(|| CommonDomain::build(&all, 0, pixel_count, cancel))
            .transpose()?;
        let pixels = SlotPixels::of_slots(slots, domain.as_ref(), dimensions, cancel)?;
        match normalization {
            Normalization::Global => global_norms(planes, &pixels, reference, cancel)?,
            Normalization::Multiplicative => {
                let medians = match domain {
                    Some(_) => slot_medians(planes, &pixels, cancel)?,
                    None => frames
                        .iter()
                        .map(|frame| frame.source_stats.medians.clone())
                        .collect(),
                };
                multiplicative_norms(&medians, reference)?
            }
            Normalization::None => unreachable!("handled by the caller"),
        }
    };
    tracing::info!(
        frame_count = frames.len(),
        slots = slots.count(),
        ref_frame = reference,
        common_domain = gated && !per_channel,
        estimator,
        ?normalization,
        "Computed normalization"
    );
    Ok(norms)
}

/// The norms of frames gathered in each channel by its own rule — a mosaic drizzled into the
/// channels of its colours — measured pair by pair: each frame against the reference over the
/// pixels both reached in the slot's channel, by the same fit [`global_norms`] makes and the same
/// ratio [`multiplicative_norms`] takes.
///
/// The pixels every frame reached are no domain for these: each frame's drops reach a colour at
/// only part of the grid — at scale 2 and pixfrac 1, a Bayer red at 9 output pixels of 16 — so
/// their intersection thins out with every frame added. A pair keeps as many as two frames share,
/// and a pair is all either fit compares. Above scale 1 a pair can still share none: two frames a
/// photosite apart put one's red where the other's green fell. The pair's domain is then empty, and
/// its estimate is `None` rather than a comparison over nothing.
///
/// Each estimate carries its gain's relative standard error: the Deming fit's for `Global`
/// ([`paired_photometric_gain`]), and for `Multiplicative` the two medians' from the measurement
/// noise alone, `√(π/2)·σ/√n` each, since both frames are read at the same pixels and their
/// structure cancels in the ratio.
fn pairwise_norms(
    planes: SlotPlanes<'_>,
    normalization: Normalization,
    reference: usize,
    cancel: &CancelToken,
) -> Result<Vec<ArrayVec<Option<SlotEstimate>, 3>>, StackError> {
    let frames = planes.frames;
    let slots = planes.slots;
    let pixel_count = planes.pixel_count;
    let slot_count = slots.count();
    let fitted = (0..frames.len() * slot_count)
        .into_par_iter()
        .map_init(RadixMedian::default, |median, pair_index| {
            let frame_index = pair_index / slot_count;
            let slot = pair_index % slot_count;
            if frame_index == reference {
                return Ok(Some(SlotEstimate::EXACT_IDENTITY));
            }
            let pair = [&frames[frame_index], &frames[reference]];
            let domain = match CommonDomain::build(&pair, slots.channel(slot), pixel_count, cancel)
            {
                Ok(domain) => domain,
                Err(StackError::NoCommonCoverage) => return Ok(None),
                Err(error) => return Err(error),
            };
            let indices = stratified_indices(&domain, cancel)?;
            let [ours, theirs] = [frame_index, reference].map(|index| {
                measure_plane(
                    planes.plane(index, slot),
                    pixel_count,
                    Some(&domain),
                    &indices,
                    median,
                    cancel,
                )
            });
            let (ours, theirs) = (ours?, theirs?);
            let measured_median = |measured: &PlaneMeasurement| {
                measured
                    .median
                    .expect("a pair's medians are measured over its domain")
            };
            let (frame_median, reference_median) =
                (measured_median(&ours), measured_median(&theirs));
            let variance = |index| {
                source_noise_variance(&frames[index], slots, slot, &indices, pixel_count, cancel)
            };
            let (frame_variance, reference_variance) =
                (variance(frame_index)?, variance(reference)?);
            match normalization {
                Normalization::Global => {
                    let paired = paired_photometric_gain(
                        &ours.samples,
                        &theirs.samples,
                        sample_stats(&theirs.samples, cancel)?,
                        frame_variance,
                        reference_variance,
                        cancel,
                    )?;
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
/// where the spread alone would read a noisier frame as a brighter one. Subtracting variances is
/// exact for independent Gaussian parts and an approximation otherwise; `σ` is the noise where no
/// flat amplified it, so on a vignetted frame the noise left in is a little more than none, alike
/// in every frame of one setup. `Global` takes the gain as the reference's scale over the frame's
/// and puts the medians on each other; `Multiplicative` takes the ratio of the medians.
///
/// A slot whose `spread² − σ²` does not clear [`SIGNAL_SIGNIFICANCE`] standard errors of its own
/// estimate holds no signal to scale by: on pure noise the difference scatters about zero, and its
/// square root would be a gain drawn from that scatter.
///
/// Each estimate carries its gain's relative standard error, the reference's and the frame's
/// added in quadrature: for `Global` each scale's, half the relative error of `spread² − σ²`; for
/// `Multiplicative` each median's, `√(π/2)·spread/√n` — the spread, not the noise, since two frames
/// measured over different pixels differ by their structure too. A frame measured without its
/// spread has no error to state, an infinite one.
///
/// # Errors
/// [`StackError::NoSignalScale`] for a `Global` slot without a significant signal spread, and as
/// [`SlotNorm::median_ratio`] for `Multiplicative`.
fn per_frame_norms(
    frames: &[StoredFrame],
    normalization: Normalization,
    reference: usize,
) -> Result<Vec<ArrayVec<SlotEstimate, 3>>, StackError> {
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
        let spread =
            spread(index, slot).expect("a frame gathered per channel was measured with its spread");
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
                    match normalization {
                        Normalization::Global => {
                            let (ours, theirs) = (scale(index, slot)?, scale(reference, slot)?);
                            let gain = (theirs.scale / ours.scale) as f32;
                            Ok(SlotEstimate {
                                norm: SlotNorm {
                                    gain,
                                    offset: medians[reference][slot] - medians[index][slot] * gain,
                                },
                                relative_error: ours.relative_error.hypot(theirs.relative_error),
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

/// A slot's signal scale, `√(spread² − σ²)`, and its relative standard error.
#[derive(Debug, Clone, Copy)]
struct MeasuredScale {
    scale: f64,
    relative_error: f64,
}

/// The largest of `errors`; 0 for none.
fn worst(errors: impl Iterator<Item = f64>) -> f64 {
    errors.fold(0.0, f64::max)
}

/// One slot's norm and its gain's relative standard error.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SlotEstimate {
    norm: SlotNorm,
    relative_error: f64,
}

impl SlotEstimate {
    /// The reference's own norm, which no estimate makes.
    const EXACT_IDENTITY: Self = Self {
        norm: SlotNorm::IDENTITY,
        relative_error: 0.0,
    };
}

/// The norms of frames gathered per channel, and which estimator gave them.
#[derive(Debug)]
struct PerChannelNorms {
    norms: Vec<FrameNorm>,
    estimator: &'static str,
}

impl PerChannelNorms {
    /// The pairwise norms when every pair shares pixels and each pair's gain is no less precise
    /// than the per-frame estimate of the same frame and slot; the per-frame norms otherwise, for
    /// every frame. The per-frame estimator also carries a bias its error does not state — two
    /// frames measured over different pixels — so a tie goes to the pairs.
    ///
    /// # Errors
    /// The per-frame estimator's, when the pairs do not stand.
    fn choose(
        pairwise: Vec<ArrayVec<Option<SlotEstimate>, 3>>,
        per_frame: Result<Vec<ArrayVec<SlotEstimate, 3>>, StackError>,
    ) -> Result<Self, StackError> {
        let shared = pairwise.iter().flatten().all(Option::is_some);
        let pairs_stand = shared
            && per_frame.as_ref().map_or(true, |per_frame| {
                pairwise.iter().zip(per_frame).all(|(pairs, singles)| {
                    pairs.iter().zip(singles).all(|(pair, single)| {
                        pair.is_some_and(|pair| pair.relative_error <= single.relative_error)
                    })
                })
            });
        let pairwise_worst = worst(
            pairwise
                .iter()
                .flatten()
                .flatten()
                .map(|pair| pair.relative_error),
        );
        if pairs_stand {
            tracing::debug!(pairwise_worst, "per-channel norms fitted pair by pair");
            return Ok(Self {
                norms: pairwise
                    .into_iter()
                    .map(|pairs| FrameNorm {
                        slots: pairs.into_iter().flatten().map(|pair| pair.norm).collect(),
                    })
                    .collect(),
                estimator: "pairwise",
            });
        }
        let per_frame = per_frame?;
        tracing::debug!(
            shared,
            pairwise_worst,
            per_frame_worst = worst(
                &mut per_frame
                    .iter()
                    .flatten()
                    .map(|single| single.relative_error)
            ),
            "per-channel norms measured frame by frame"
        );
        Ok(Self {
            norms: per_frame
                .into_iter()
                .map(|singles| FrameNorm {
                    slots: singles.into_iter().map(|single| single.norm).collect(),
                })
                .collect(),
            estimator: "per-frame",
        })
    }
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

/// The least noisy frame: the lowest mean noise variance over its slots, each σ carried into the
/// shared domain by `to_domain`, so frames decoded at different scales compare in one unit. The
/// first wins a tie.
fn select_reference_frame(frames: &[StoredFrame], to_domain: &[DomainMap]) -> usize {
    let mean_variance = |(frame, map): (&StoredFrame, &DomainMap)| {
        let stats = &frame.source_stats;
        (0..stats.medians.len())
            .map(|slot| (map.gain * f64::from(stats.slot_noise(slot))).powi(2))
            .sum::<f64>()
            / stats.medians.len() as f64
    };
    let mut scores = frames.iter().zip(to_domain).map(mean_variance).enumerate();
    let (_, mut best_score) = scores.next().expect("normalization requires frames");
    let mut best_frame = 0;
    for (frame, score) in scores {
        if score < best_score {
            best_score = score;
            best_frame = frame;
        }
    }
    best_frame
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

/// The frames' planes, read by slot.
#[derive(Debug, Clone, Copy)]
struct SlotPlanes<'a> {
    frames: &'a [StoredFrame],
    slots: Slots,
    pixel_count: usize,
}

impl SlotPlanes<'_> {
    /// The plane holding `slot` of `frame`.
    fn plane(&self, frame: usize, slot: usize) -> &StoredPlane {
        &self.frames[frame].channels[self.slots.channel(slot)]
    }
}

/// Every frame's slot medians over the pixels the frames share, one pass per slot.
fn slot_medians(
    planes: SlotPlanes<'_>,
    pixels: &[SlotPixels<'_>],
    cancel: &CancelToken,
) -> Result<Vec<ArrayVec<f32, 3>>, StackError> {
    let slot_count = pixels.len();
    let medians = (0..planes.frames.len() * slot_count)
        .into_par_iter()
        .map_init(RadixMedian::default, |median, pair_index| {
            let slot = pair_index % slot_count;
            let measured = measure_plane(
                planes.plane(pair_index / slot_count, slot),
                planes.pixel_count,
                pixels[slot].shared(),
                &[],
                median,
                cancel,
            )?;
            Ok(measured.median.expect("the frames share only some pixels"))
        })
        .collect::<Result<Vec<_>, StackError>>()?;
    Ok(medians
        .chunks(slot_count)
        .map(|slots| slots.iter().copied().collect())
        .collect())
}

/// Fit every frame's gain and offset against the reference frame directly, slot by slot.
///
/// Each non-reference frame is paired with the reference through [`paired_photometric_gain`]'s
/// errors-in-variables fit, on a stratified sample of the slot's measured pixels; the offset then
/// puts the frame's median on the reference's. The reference itself is the identity by definition.
///
/// Fitting against the reference rather than against frame 0 and rescaling afterwards matters:
/// the fit clips residuals and weights each side by its own noise, so `gain(a→c)` is not
/// `gain(a→b) / gain(c→b)` — chaining through an arbitrary frame would put its noise into every
/// other frame's scale.
fn global_norms(
    planes: SlotPlanes<'_>,
    pixels: &[SlotPixels<'_>],
    reference: usize,
    cancel: &CancelToken,
) -> Result<Vec<FrameNorm>, StackError> {
    let frames = planes.frames;
    let pixel_count = planes.pixel_count;
    let slot_count = pixels.len();
    let indices = pixels
        .iter()
        .map(|pixels| pixels.indices(cancel))
        .collect::<Result<ArrayVec<_, 3>, StackError>>()?;
    let measure = |frame: usize, slot: usize, median: &mut RadixMedian| {
        if let (SlotPixels::Everywhere(_), Some(samples)) = (&pixels[slot], &frames[frame].samples)
        {
            let samples = &samples[slot];
            debug_assert_eq!(samples.samples(), indices[slot].len());
            return Ok(PlaneMeasurement {
                median: None,
                samples: samples.chunk(0, samples.samples()).to_vec(),
            });
        }
        measure_plane(
            planes.plane(frame, slot),
            pixel_count,
            pixels[slot].shared(),
            &indices[slot],
            median,
            cancel,
        )
    };
    let reference_slots = (0..slot_count)
        .into_par_iter()
        .map(|slot| {
            let frame = &frames[reference];
            let measured = measure(reference, slot, &mut RadixMedian::default())?;
            Ok(ReferenceSlot {
                median: measured.median.unwrap_or(frame.source_stats.medians[slot]),
                stats: sample_stats(&measured.samples, cancel)?,
                samples: measured.samples,
                noise_variance: source_noise_variance(
                    frame,
                    planes.slots,
                    slot,
                    &indices[slot],
                    pixel_count,
                    cancel,
                )?,
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
            let gain = paired_photometric_gain(
                &measured.samples,
                &reference.samples,
                reference.stats,
                source_noise_variance(
                    frame,
                    planes.slots,
                    slot,
                    &indices[slot],
                    pixel_count,
                    cancel,
                )?,
                reference.noise_variance,
                cancel,
            )?
            .gain;
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

/// `plane`'s median over `domain` when one is given, and its values at the ascending `indices`.
///
/// The median is exact and holds no copy of the plane: `median`'s histogram, which the caller
/// reuses across planes, ranks the domain's values in two walks over it. A copy for every plane
/// measured at once would be a plane per worker, which no memory plan charges.
fn measure_plane(
    plane: &StoredPlane,
    pixel_count: usize,
    domain: Option<&CommonDomain>,
    indices: &[usize],
    median: &mut RadixMedian,
    cancel: &CancelToken,
) -> Result<PlaneMeasurement, StackError> {
    debug_assert!(indices.is_sorted(), "the sample indices ascend");
    let values = plane.chunk(0, pixel_count);
    let mut samples = Vec::with_capacity(indices.len());
    for chunk in indices.chunks(Cancelled::POLL_CHUNK) {
        Cancelled::check(cancel)?;
        samples.extend(chunk.iter().map(|&index| values[index]));
    }
    let median = match domain {
        Some(domain) => {
            let mut high = median.high_pass();
            for_each_in_domain(values, domain, cancel, |value| high.add(value))?;
            debug_assert_eq!(
                high.len(),
                domain.sample_count,
                "the walk visits the domain"
            );
            let mut low = high.finish();
            for_each_in_domain(values, domain, cancel, |value| low.add(value))?;
            Some(low.median())
        }
        None => None,
    };
    Ok(PlaneMeasurement { median, samples })
}

/// Call `visit` with every value of `values` the domain holds, in pixel order.
fn for_each_in_domain(
    values: &[f32],
    domain: &CommonDomain,
    cancel: &CancelToken,
    mut visit: impl FnMut(f32),
) -> Result<(), StackError> {
    for (chunk, chunk_values) in values.chunks(Cancelled::POLL_CHUNK).enumerate() {
        Cancelled::check(cancel)?;
        let base = chunk * Cancelled::POLL_CHUNK;
        let end = base + chunk_values.len();
        // The mask is one row, so word `w` covers pixels `64w..64w + 64`, and a chunk starts on a
        // word: its words are read once each instead of a bit lookup per pixel.
        let words = &domain.valid.words[base / WORD_BITS..end.div_ceil(WORD_BITS)];
        for (offset, &word) in words.iter().enumerate() {
            let word_base = offset * WORD_BITS;
            let mut bits = word;
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                visit(chunk_values[word_base + bit]);
            }
        }
    }
    Ok(())
}

/// Up to [`SAMPLE_LIMIT`] pixel indices, ascending and evenly spread by rank over the pixels
/// `domain` holds, as [`StratifiedSamples`] spreads them over every pixel: the `k`-th of `m` is the
/// one of rank `⌊k·n/m⌋` among the `n`.
fn stratified_indices(
    domain: &CommonDomain,
    cancel: &CancelToken,
) -> Result<Vec<usize>, StackError> {
    let sample_count = domain.sample_count;
    let retained = sample_count.min(SAMPLE_LIMIT);
    let mut indices = Vec::with_capacity(retained);
    let mut rank = 0;
    // The mask is one row, so word `w` covers pixels `64w..64w + 64`; the cancel poll runs per
    // word group rather than per pixel.
    for (group, words) in domain
        .valid
        .words
        .chunks(Cancelled::POLL_CHUNK / WORD_BITS)
        .enumerate()
    {
        Cancelled::check(cancel)?;
        for (offset, &word) in words.iter().enumerate() {
            let base = (group * (Cancelled::POLL_CHUNK / WORD_BITS) + offset) * WORD_BITS;
            let mut bits = word;
            while bits != 0 && indices.len() < retained {
                let bit = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                if rank == indices.len() * sample_count / retained {
                    indices.push(base + bit);
                }
                rank += 1;
            }
        }
    }
    debug_assert_eq!(indices.len(), retained);
    Ok(indices)
}

/// The noise variance of one frame's slot at the sampled pixels: the source's white noise σ²,
/// scaled by the mean inverse confidence there, since interpolation that averaged several source
/// pixels left less noise than the source had.
fn source_noise_variance(
    frame: &StoredFrame,
    slots: Slots,
    slot: usize,
    indices: &[usize],
    pixel_count: usize,
    cancel: &CancelToken,
) -> Result<f64, StackError> {
    let sigma = f64::from(frame.source_stats.slot_noise(slot));
    let Some(confidence) = frame.quality.confidence(slots.channel(slot)) else {
        return Ok(sigma * sigma);
    };
    let values = confidence.chunk(0, pixel_count);
    let mut inverse_confidence = 0.0;
    for chunk in indices.chunks(Cancelled::POLL_CHUNK) {
        Cancelled::check(cancel)?;
        for &index in chunk {
            let value = f64::from(values[index]);
            // `indices` are common-domain pixels, which clear the coverage floor, and a
            // frame-quality pair has confidence wherever it has support — so this is never a
            // division by zero.
            debug_assert!(
                value > 0.0,
                "zero confidence at common-domain pixel {index}"
            );
            inverse_confidence += 1.0 / value;
        }
    }
    Ok(sigma * sigma * inverse_confidence / indices.len() as f64)
}

#[cfg(test)]
mod tests;
