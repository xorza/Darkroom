//! Putting every frame on one photometric scale before they are combined.
//!
//! Frames of the same field differ in sky level and transparency, so combining them raw would let
//! the brightest dominate and turn rejection into a vote about exposure rather than about
//! outliers. Each frame gets an affine `gain`/`offset` per channel measured against a reference —
//! the least noisy of the set — and the combine applies it as it gathers.
//!
//! `Normalization::Global` fits every frame's gain against the reference by
//! [`photometric_gain`]'s errors-in-variables fit over paired pixels, and its offset from the two
//! medians. `Normalization::Multiplicative` takes the ratio of medians. Which pixels the
//! statistics come from is the one thing coverage changes: frames that each cover every pixel are
//! measured over all of them, and when any frame contributes at only some pixels — a warp's edge,
//! or a source's declared nulls — over [`common_domain`], the pixels every frame reached.

pub(crate) mod common_domain;
pub(crate) mod photometric_gain;

use arrayvec::ArrayVec;
use common::CancelToken;
use rayon::prelude::*;

use crate::combine::CANCEL_POLL_CHUNK;
use crate::combine::config::Normalization;
use crate::combine::error::StackError;
use crate::combine::normalization::common_domain::CommonDomain;
use crate::combine::normalization::photometric_gain::{paired_photometric_gain, sample_stats};
use crate::frame_store::stored_frame::StoredFrame;
use crate::frame_store::stored_plane::StoredPlane;
use crate::io::cancelled::Cancelled;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::sample_domain::DomainMap;
use crate::math::statistics::{MedianMad, median_mut};
use std::iter;

/// Per-channel affine normalization applied as `normalized = raw * gain + offset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ChannelNorm {
    pub(crate) gain: f32,
    pub(crate) offset: f32,
}

impl ChannelNorm {
    const IDENTITY: Self = Self {
        gain: 1.0,
        offset: 0.0,
    };
}

/// Per-frame affine normalization parameters.
#[derive(Debug, Clone)]
pub(crate) struct FrameNorm {
    pub(crate) channels: ArrayVec<ChannelNorm, 3>,
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
        normalization: Normalization,
        cancel: &CancelToken,
    ) -> Result<Option<Vec<Self>>, StackError> {
        let to_domain = domain_maps(frames);
        if normalization == Normalization::None {
            return Ok(to_domain
                .iter()
                .any(|&map| map != DomainMap::IDENTITY)
                .then(|| {
                    frames
                        .iter()
                        .zip(&to_domain)
                        .map(|(frame, map)| FrameNorm {
                            channels: (0..frame.source_stats.channels.len())
                                .map(|_| ChannelNorm {
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
        let mut norms = fitted_frame_norms(frames, dimensions, normalization, reference, cancel)?;
        // A fitted norm lands each frame on the reference frame's raw values; the reference's own
        // map then carries those into the shared domain: `map(gain·x + offset)`.
        let map = to_domain[reference];
        if map != DomainMap::IDENTITY {
            for channel in norms.iter_mut().flat_map(|norm| norm.channels.iter_mut()) {
                channel.gain = (map.gain * f64::from(channel.gain)) as f32;
                channel.offset = (map.gain * f64::from(channel.offset) + map.offset) as f32;
            }
        }
        Ok(Some(norms))
    }
}

/// One channel of the reference frame, measured once and paired against every other frame.
#[derive(Debug)]
struct ReferenceChannel {
    median: f32,
    samples: Vec<f32>,
    stats: MedianMad,
    noise_variance: f64,
}

/// What one pass over a plane measures: its median over the common domain when there is one, and
/// its values at the stratified sample indices.
#[derive(Debug)]
struct PlaneMeasurement {
    median: Option<f32>,
    samples: Vec<f32>,
}

/// Paired samples the gain fit runs on, at most: a stratified 65 536 of the measured pixels.
const PHOTOMETRIC_SAMPLE_LIMIT: usize = 65_536;

/// Pixels per word of the common-domain mask.
const WORD_BITS: usize = u64::BITS as usize;
const _: () = assert!(
    CANCEL_POLL_CHUNK.is_multiple_of(WORD_BITS),
    "a gather chunk starts on a mask word"
);

/// The map that expresses each frame in the domain of the first frame declaring one; the identity
/// for a frame that declares none, or when none does.
///
/// The combine's constructors admit the set's facts first ([`SetFacts`]), so every declared domain
/// converts.
///
/// [`SetFacts`]: crate::combine::cache::set_facts::SetFacts
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
/// different set of pixels than the one the frames share.
fn fitted_frame_norms(
    frames: &[StoredFrame],
    dimensions: ImageDimensions,
    normalization: Normalization,
    reference: usize,
    cancel: &CancelToken,
) -> Result<Vec<FrameNorm>, StackError> {
    let pixel_count = dimensions.pixel_count();
    let domain = frames
        .iter()
        .any(|frame| !frame.quality.is_none())
        .then(|| CommonDomain::build(frames, pixel_count, cancel))
        .transpose()?;
    let norms = match normalization {
        Normalization::Global => {
            global_norms(frames, pixel_count, domain.as_ref(), reference, cancel)?
        }
        Normalization::Multiplicative => {
            let medians = match &domain {
                Some(domain) => domain_medians(frames, pixel_count, domain, cancel)?,
                None => frames.iter().map(source_medians).collect(),
            };
            multiplicative_norms(&medians, reference)?
        }
        Normalization::None => unreachable!("handled by the caller"),
    };
    tracing::info!(
        frame_count = frames.len(),
        channels = frames[0].channels.len(),
        ref_frame = reference,
        common_domain = domain.is_some(),
        ?normalization,
        "Computed normalization"
    );
    Ok(norms)
}

/// The least noisy frame: the lowest mean noise variance over its channels, each σ carried into
/// the shared domain by `to_domain`, so frames decoded at different scales compare in one unit.
/// The first wins a tie.
fn select_reference_frame(frames: &[StoredFrame], to_domain: &[DomainMap]) -> usize {
    let mean_variance = |(frame, map): (&StoredFrame, &DomainMap)| {
        let stats = &frame.source_stats;
        (0..stats.channels.len())
            .map(|channel| (map.gain * f64::from(stats.channel_noise(channel))).powi(2))
            .sum::<f64>()
            / stats.channels.len() as f64
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

fn source_medians(frame: &StoredFrame) -> ArrayVec<f32, 3> {
    frame
        .source_stats
        .channels
        .iter()
        .map(|channel| channel.median)
        .collect()
}

fn identity_norm(channel_count: usize) -> FrameNorm {
    let mut channels = ArrayVec::new();
    channels.extend(iter::repeat_n(ChannelNorm::IDENTITY, channel_count));
    FrameNorm { channels }
}

/// `gain = median_ref / median`, per channel.
///
/// # Errors
/// [`StackError::NonPositiveMedian`] when a median is not positive: a ratio to it scales nothing, and
/// unit gain in its place would combine the frame at a scale no one measured.
fn multiplicative_norms(
    medians: &[ArrayVec<f32, 3>],
    reference: usize,
) -> Result<Vec<FrameNorm>, StackError> {
    medians
        .iter()
        .enumerate()
        .map(|(index, frame)| {
            let channels = frame
                .iter()
                .zip(&medians[reference])
                .enumerate()
                .map(|(channel, (&median, &reference_median))| {
                    if median > 0.0 && reference_median > 0.0 {
                        Ok(ChannelNorm {
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
                            channel,
                            median,
                        })
                    }
                })
                .collect::<Result<_, _>>()?;
            Ok(FrameNorm { channels })
        })
        .collect()
}

/// Every frame's channel medians over the common domain, one pass per plane.
fn domain_medians(
    frames: &[StoredFrame],
    pixel_count: usize,
    domain: &CommonDomain,
    cancel: &CancelToken,
) -> Result<Vec<ArrayVec<f32, 3>>, StackError> {
    let channel_count = frames[0].channels.len();
    let medians = (0..frames.len() * channel_count)
        .into_par_iter()
        .map_init(Vec::new, |buffer, pair_index| {
            let plane = &frames[pair_index / channel_count].channels[pair_index % channel_count];
            let measured = measure_plane(plane, pixel_count, Some(domain), &[], buffer, cancel)?;
            Ok(measured.median.expect("a domain was given"))
        })
        .collect::<Result<Vec<_>, StackError>>()?;
    Ok(medians
        .chunks(channel_count)
        .map(|channels| channels.iter().copied().collect())
        .collect())
}

/// Fit every frame's gain and offset against the reference frame directly.
///
/// Each non-reference frame is paired with the reference through [`paired_photometric_gain`]'s
/// errors-in-variables fit, on a stratified sample of the measured pixels; the offset then puts
/// the frame's median on the reference's. The reference itself is the identity by definition.
///
/// Every plane is read once: the pass that gathers its median also takes its samples. Fitting
/// against the reference rather than against frame 0 and rescaling afterwards matters: the fit
/// clips residuals and weights each side by its own noise, so `gain(a→c)` is not
/// `gain(a→b) / gain(c→b)` — chaining through an arbitrary frame would put its noise into every
/// other frame's scale.
fn global_norms(
    frames: &[StoredFrame],
    pixel_count: usize,
    domain: Option<&CommonDomain>,
    reference: usize,
    cancel: &CancelToken,
) -> Result<Vec<FrameNorm>, StackError> {
    let channel_count = frames[0].channels.len();
    let indices = stratified_indices(pixel_count, domain, cancel)?;
    let reference_channels = (0..channel_count)
        .into_par_iter()
        .map(|channel| {
            let frame = &frames[reference];
            let mut buffer = Vec::new();
            let measured = measure_plane(
                &frame.channels[channel],
                pixel_count,
                domain,
                &indices,
                &mut buffer,
                cancel,
            )?;
            Ok(ReferenceChannel {
                median: measured
                    .median
                    .unwrap_or(frame.source_stats.channels[channel].median),
                stats: sample_stats(&measured.samples, cancel)?,
                samples: measured.samples,
                noise_variance: source_noise_variance(
                    frame,
                    channel,
                    &indices,
                    pixel_count,
                    cancel,
                )?,
            })
        })
        .collect::<Result<Vec<_>, StackError>>()?;

    let fitted = (0..frames.len() * channel_count)
        .into_par_iter()
        .map_init(Vec::new, |buffer, pair_index| {
            let frame_index = pair_index / channel_count;
            let channel = pair_index % channel_count;
            if frame_index == reference {
                return Ok(ChannelNorm::IDENTITY);
            }
            let frame = &frames[frame_index];
            let measured = measure_plane(
                &frame.channels[channel],
                pixel_count,
                domain,
                &indices,
                buffer,
                cancel,
            )?;
            let median = measured
                .median
                .unwrap_or(frame.source_stats.channels[channel].median);
            let reference = &reference_channels[channel];
            let gain = paired_photometric_gain(
                &measured.samples,
                &reference.samples,
                reference.stats,
                source_noise_variance(frame, channel, &indices, pixel_count, cancel)?,
                reference.noise_variance,
                cancel,
            )?;
            Ok(ChannelNorm {
                gain,
                offset: reference.median - median * gain,
            })
        })
        .collect::<Result<Vec<_>, StackError>>()?;

    let mut norms = frames
        .iter()
        .map(|frame| identity_norm(frame.channels.len()))
        .collect::<Vec<_>>();
    for (frame_index, channels) in fitted.chunks(channel_count).enumerate() {
        for (channel, &norm) in channels.iter().enumerate() {
            norms[frame_index].channels[channel] = norm;
        }
    }
    Ok(norms)
}

/// One walk over `plane`: its median over `domain` when one is given — gathered into `buffer`,
/// which the caller reuses across planes — and its values at the ascending `indices`.
fn measure_plane(
    plane: &StoredPlane,
    pixel_count: usize,
    domain: Option<&CommonDomain>,
    indices: &[usize],
    buffer: &mut Vec<f32>,
    cancel: &CancelToken,
) -> Result<PlaneMeasurement, StackError> {
    debug_assert!(indices.is_sorted(), "the sample indices ascend");
    let values = plane.chunk(0, pixel_count);
    buffer.clear();
    if let Some(domain) = domain {
        buffer.reserve_exact(domain.sample_count);
    }
    let mut samples = Vec::with_capacity(indices.len());
    let mut next = 0;
    for (chunk, chunk_values) in values.chunks(CANCEL_POLL_CHUNK).enumerate() {
        Cancelled::check(cancel)?;
        let base = chunk * CANCEL_POLL_CHUNK;
        let end = base + chunk_values.len();
        while next < indices.len() && indices[next] < end {
            samples.push(chunk_values[indices[next] - base]);
            next += 1;
        }
        if let Some(domain) = domain {
            // The mask is one row, so word `w` covers pixels `64w..64w + 64`, and a chunk starts
            // on a word: its words are read once each instead of a bit lookup per pixel.
            let words = &domain.valid.words[base / WORD_BITS..end.div_ceil(WORD_BITS)];
            for (offset, &word) in words.iter().enumerate() {
                let word_base = offset * WORD_BITS;
                let mut bits = word;
                while bits != 0 {
                    let bit = bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    buffer.push(chunk_values[word_base + bit]);
                }
            }
        }
    }
    let median = match domain {
        Some(_) => {
            Cancelled::check(cancel)?;
            Some(median_mut(buffer))
        }
        None => None,
    };
    Ok(PlaneMeasurement { median, samples })
}

/// Up to [`PHOTOMETRIC_SAMPLE_LIMIT`] pixel indices, ascending and evenly spread by rank over the
/// measured pixels — every pixel, or the common domain's: the `k`-th of `m` is the one of rank
/// `⌊k·n/m⌋` among the `n`.
fn stratified_indices(
    pixel_count: usize,
    domain: Option<&CommonDomain>,
    cancel: &CancelToken,
) -> Result<Vec<usize>, StackError> {
    let Some(domain) = domain else {
        let retained = pixel_count.min(PHOTOMETRIC_SAMPLE_LIMIT);
        return Ok((0..retained).map(|k| k * pixel_count / retained).collect());
    };
    let sample_count = domain.sample_count;
    let retained = sample_count.min(PHOTOMETRIC_SAMPLE_LIMIT);
    let mut indices = Vec::with_capacity(retained);
    let mut rank = 0;
    // The mask is one row, so word `w` covers pixels `64w..64w + 64`; the cancel poll runs per
    // word group rather than per pixel.
    for (group, words) in domain
        .valid
        .words
        .chunks(CANCEL_POLL_CHUNK / WORD_BITS)
        .enumerate()
    {
        Cancelled::check(cancel)?;
        for (offset, &word) in words.iter().enumerate() {
            let base = (group * (CANCEL_POLL_CHUNK / WORD_BITS) + offset) * WORD_BITS;
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

/// The noise variance of one frame's channel at the sampled pixels: the source's white noise σ²,
/// scaled by the mean inverse confidence there, since interpolation that averaged several source
/// pixels left less noise than the source had.
fn source_noise_variance(
    frame: &StoredFrame,
    channel: usize,
    indices: &[usize],
    pixel_count: usize,
    cancel: &CancelToken,
) -> Result<f64, StackError> {
    let sigma = f64::from(frame.source_stats.channel_noise(channel));
    let Some(confidence) = frame.quality.confidence() else {
        return Ok(sigma * sigma);
    };
    let values = confidence.chunk(0, pixel_count);
    let mut inverse_confidence = 0.0;
    for chunk in indices.chunks(CANCEL_POLL_CHUNK) {
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
