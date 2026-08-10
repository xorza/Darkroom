//! Putting every frame on one photometric scale before they are combined.
//!
//! Frames of the same field differ in sky level and transparency, so combining them raw would let
//! the brightest dominate and turn rejection into a vote about exposure rather than about
//! outliers. Each frame gets an affine `gain`/`offset` per channel measured against a reference —
//! the least noisy of the set — and the combine applies it as it gathers.
//!
//! How that is measured depends on the frames. Unregistered frames already share a pixel grid, so
//! their whole-image median and MAD are directly comparable. Registered ones do not: each covers a
//! slightly different patch of sky, so the measurement is confined to [`common_domain`], the
//! pixels every frame actually reached, and the gain comes from [`photometric_gain`]'s
//! errors-in-variables fit rather than a ratio of spreads.

pub(crate) mod common_domain;
pub(crate) mod photometric_gain;

use arrayvec::ArrayVec;
use common::CancelToken;
use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::math::statistics::{MedianMad, mad_to_sigma, median_f32_mut};
use crate::stacking::combine::config::Normalization;
use crate::stacking::combine::error::Error;
use crate::stacking::combine::error::check_cancel;
use crate::stacking::combine::normalization::common_domain::CommonDomain;
use crate::stacking::combine::normalization::photometric_gain::{
    paired_photometric_gain, sample_stats,
};
use crate::stacking::frame_store::StoredFrame;
use crate::stacking::frame_store::frame_stats::FrameStats;
use crate::stacking::frame_store::stored_plane::StoredPlane;

/// Per-channel affine normalization applied as `normalized = raw * gain + offset`.
#[derive(Debug, Clone, Copy)]
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

#[derive(Debug)]
enum RegisteredMeasurements {
    CommonStats(Vec<FrameStats>),
    GlobalNormsToFirst(Vec<FrameNorm>),
}

#[derive(Debug)]
struct ReferenceFit {
    samples: Vec<f32>,
    stats: MedianMad,
    noise_variance: f64,
}

const PHOTOMETRIC_SAMPLE_LIMIT: usize = 65_536;
pub(super) const NORMALIZATION_CHUNK_SIZE: usize = 16_384;

pub(crate) fn compute_frame_norms(
    frames: &[StoredFrame],
    dimensions: ImageDimensions,
    normalization: Normalization,
    cancel: &CancelToken,
) -> Result<Option<Vec<FrameNorm>>, Error> {
    if normalization == Normalization::None {
        return Ok(None);
    }
    check_cancel(cancel)?;

    let reference = select_reference_frame(frames.iter().map(|frame| &frame.source_stats));
    let registered = frames.iter().any(|frame| !frame.quality.is_none());
    if !registered {
        let norms = compute_frame_norms_with_reference(
            frames.iter().map(|frame| &frame.source_stats),
            normalization,
            reference,
        );
        check_cancel(cancel)?;
        return Ok(Some(norms));
    }

    match measure_registered_frames(frames, dimensions, normalization, cancel)? {
        RegisteredMeasurements::GlobalNormsToFirst(mut norms) => {
            let reference_norm = norms[reference].clone();
            for (frame_index, frame_norm) in norms.iter_mut().enumerate() {
                check_cancel(cancel)?;
                if frame_index == reference {
                    frame_norm.channels.fill(ChannelNorm::IDENTITY);
                    continue;
                }
                for (channel, norm) in frame_norm.channels.iter_mut().enumerate() {
                    let reference_channel = reference_norm.channels[channel];
                    norm.gain /= reference_channel.gain;
                    norm.offset = (norm.offset - reference_channel.offset) / reference_channel.gain;
                }
            }
            Ok(Some(norms))
        }
        RegisteredMeasurements::CommonStats(stats) => Ok(Some(compute_frame_norms_with_reference(
            stats.iter(),
            normalization,
            reference,
        ))),
    }
}

fn select_reference_frame<'a>(stats: impl IntoIterator<Item = &'a FrameStats>) -> usize {
    let mut stats = stats.into_iter().enumerate();
    let (_, first) = stats.next().expect("normalization requires frames");
    let mut best_frame = 0;
    let mut best_mad = average_mad(first);

    for (frame_index, frame_stats) in stats {
        let average_mad = average_mad(frame_stats);
        if average_mad < best_mad {
            best_mad = average_mad;
            best_frame = frame_index;
        }
    }
    best_frame
}

fn average_mad(stats: &FrameStats) -> f32 {
    stats
        .channels
        .iter()
        .map(|channel| channel.mad)
        .sum::<f32>()
        / stats.channels.len() as f32
}

fn compute_frame_norms_with_reference<'a>(
    stats: impl IntoIterator<Item = &'a FrameStats>,
    normalization: Normalization,
    reference: usize,
) -> Vec<FrameNorm> {
    assert_ne!(normalization, Normalization::None);
    let stats: Vec<&FrameStats> = stats.into_iter().collect();
    let channels = stats[0].channels.len();
    let mut norms: Vec<FrameNorm> = stats
        .iter()
        .map(|stats| identity_norm(stats.channels.len()))
        .collect();

    for channel in 0..channels {
        let MedianMad {
            median: reference_median,
            mad: reference_mad,
        } = stats[reference].channels[channel];

        for (frame_index, frame_stats) in stats.iter().enumerate() {
            if frame_index == reference {
                continue;
            }
            let MedianMad {
                median: frame_median,
                mad: frame_mad,
            } = frame_stats.channels[channel];
            norms[frame_index].channels[channel] = match normalization {
                Normalization::Global => {
                    let gain = if frame_mad > f32::EPSILON {
                        reference_mad / frame_mad
                    } else {
                        1.0
                    };
                    ChannelNorm {
                        gain,
                        offset: reference_median - frame_median * gain,
                    }
                }
                Normalization::Multiplicative => {
                    let gain = if frame_median > f32::EPSILON {
                        reference_median / frame_median
                    } else {
                        1.0
                    };
                    ChannelNorm { gain, offset: 0.0 }
                }
                Normalization::None => unreachable!(),
            };
        }
    }

    tracing::info!(
        frame_count = stats.len(),
        channels,
        ref_frame = reference,
        ?normalization,
        "Computed normalization"
    );
    norms
}

fn identity_norm(channel_count: usize) -> FrameNorm {
    let mut channels = ArrayVec::new();
    channels.extend(std::iter::repeat_n(ChannelNorm::IDENTITY, channel_count));
    FrameNorm { channels }
}

fn measure_registered_frames(
    frames: &[StoredFrame],
    dimensions: ImageDimensions,
    normalization: Normalization,
    cancel: &CancelToken,
) -> Result<RegisteredMeasurements, Error> {
    let pixel_count = dimensions.pixel_count();
    let common_domain = CommonDomain::build(frames, pixel_count, cancel)?;
    let common_stats = measure_common_stats(frames, pixel_count, &common_domain, cancel)?;

    Ok(match normalization {
        Normalization::Global => {
            RegisteredMeasurements::GlobalNormsToFirst(measure_global_norms_to_first(
                frames,
                &common_stats,
                pixel_count,
                &common_domain,
                cancel,
            )?)
        }
        Normalization::Multiplicative => RegisteredMeasurements::CommonStats(common_stats),
        Normalization::None => unreachable!(),
    })
}

fn measure_common_stats(
    frames: &[StoredFrame],
    pixel_count: usize,
    common_domain: &CommonDomain,
    cancel: &CancelToken,
) -> Result<Vec<FrameStats>, Error> {
    let channel_count = frames[0].channels.len();
    let measured = (0..frames.len() * channel_count)
        .into_par_iter()
        .map_init(
            || Vec::with_capacity(common_domain.sample_count),
            |samples, pair_index| {
                let frame_index = pair_index / channel_count;
                let channel = pair_index % channel_count;
                gather_valid_samples(
                    samples,
                    &frames[frame_index].channels[channel],
                    &common_domain.valid,
                    pixel_count,
                    cancel,
                )?;
                cancellable_median_mad(samples, cancel)
            },
        )
        .collect::<Result<Vec<_>, Error>>()?;

    Ok(frames
        .iter()
        .zip(measured.chunks(channel_count))
        .map(|(frame, channels)| FrameStats {
            channels: channels.iter().copied().collect(),
            quantization_sigma: frame.source_stats.quantization_sigma,
        })
        .collect())
}

fn measure_global_norms_to_first(
    frames: &[StoredFrame],
    common_stats: &[FrameStats],
    pixel_count: usize,
    common_domain: &CommonDomain,
    cancel: &CancelToken,
) -> Result<Vec<FrameNorm>, Error> {
    let channel_count = frames[0].channels.len();
    let indices =
        stratified_valid_indices(&common_domain.valid, common_domain.sample_count, cancel)?;
    let reference_fits = (0..channel_count)
        .into_par_iter()
        .map(|channel| {
            let samples = gather_indexed_samples(
                &frames[0].channels[channel],
                &indices,
                pixel_count,
                cancel,
            )?;
            let stats = sample_stats(&samples, cancel)?;
            let noise_variance =
                source_noise_variance(&frames[0], channel, &indices, pixel_count, cancel)?;
            Ok(ReferenceFit {
                samples,
                stats,
                noise_variance,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;

    let fitted = (0..(frames.len() - 1) * channel_count)
        .into_par_iter()
        .map(|pair_index| {
            let frame_index = pair_index / channel_count + 1;
            let channel = pair_index % channel_count;
            let frame_samples = gather_indexed_samples(
                &frames[frame_index].channels[channel],
                &indices,
                pixel_count,
                cancel,
            )?;
            let reference = &reference_fits[channel];
            let gain = paired_photometric_gain(
                &frame_samples,
                &reference.samples,
                reference.stats,
                source_noise_variance(
                    &frames[frame_index],
                    channel,
                    &indices,
                    pixel_count,
                    cancel,
                )?,
                reference.noise_variance,
                cancel,
            )?;
            Ok(ChannelNorm {
                gain,
                offset: common_stats[0].channels[channel].median
                    - common_stats[frame_index].channels[channel].median * gain,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;

    let mut norms = frames
        .iter()
        .map(|frame| identity_norm(frame.channels.len()))
        .collect::<Vec<_>>();
    for (frame_index, channels) in fitted.chunks(channel_count).enumerate() {
        for (channel, &norm) in channels.iter().enumerate() {
            norms[frame_index + 1].channels[channel] = norm;
        }
    }
    Ok(norms)
}

fn gather_valid_samples(
    samples: &mut Vec<f32>,
    plane: &StoredPlane,
    common_domain: &BitBuffer2,
    pixel_count: usize,
    cancel: &CancelToken,
) -> Result<(), Error> {
    samples.clear();
    let values = plane.chunk(0, pixel_count);
    for (start, value_chunk) in values.chunks(NORMALIZATION_CHUNK_SIZE).enumerate() {
        check_cancel(cancel)?;
        let base = start * NORMALIZATION_CHUNK_SIZE;
        samples.extend(
            value_chunk
                .iter()
                .enumerate()
                .filter_map(|(offset, &value)| common_domain.get(base + offset).then_some(value)),
        );
    }
    Ok(())
}

fn stratified_valid_indices(
    common_domain: &BitBuffer2,
    sample_count: usize,
    cancel: &CancelToken,
) -> Result<Vec<usize>, Error> {
    let retained = sample_count.min(PHOTOMETRIC_SAMPLE_LIMIT);
    let mut indices = Vec::with_capacity(retained);
    let mut valid_rank = 0;
    for (pixel, valid) in common_domain.iter().enumerate() {
        if pixel % NORMALIZATION_CHUNK_SIZE == 0 {
            check_cancel(cancel)?;
        }
        if !valid {
            continue;
        }
        if indices.len() < retained && valid_rank == indices.len() * sample_count / retained {
            indices.push(pixel);
        }
        valid_rank += 1;
    }
    debug_assert_eq!(indices.len(), retained);
    Ok(indices)
}

fn gather_indexed_samples(
    plane: &StoredPlane,
    indices: &[usize],
    pixel_count: usize,
    cancel: &CancelToken,
) -> Result<Vec<f32>, Error> {
    let pixels = plane.chunk(0, pixel_count);
    let mut samples = Vec::with_capacity(indices.len());
    for chunk in indices.chunks(NORMALIZATION_CHUNK_SIZE) {
        check_cancel(cancel)?;
        samples.extend(chunk.iter().map(|&index| pixels[index]));
    }
    Ok(samples)
}

fn source_noise_variance(
    frame: &StoredFrame,
    channel: usize,
    indices: &[usize],
    pixel_count: usize,
    cancel: &CancelToken,
) -> Result<f64, Error> {
    let sigma = f64::from(mad_to_sigma(frame.source_stats.channels[channel].mad));
    let Some(confidence) = &frame.quality.confidence else {
        return Ok(sigma * sigma);
    };
    let values = confidence.chunk(0, pixel_count);
    let mut inverse_confidence = 0.0;
    for chunk in indices.chunks(NORMALIZATION_CHUNK_SIZE) {
        check_cancel(cancel)?;
        for &index in chunk {
            inverse_confidence += 1.0 / f64::from(values[index]);
        }
    }
    Ok(sigma * sigma * inverse_confidence / indices.len() as f64)
}

pub(super) fn cancellable_median_mad(
    samples: &mut [f32],
    cancel: &CancelToken,
) -> Result<MedianMad, Error> {
    check_cancel(cancel)?;
    let median = median_f32_mut(samples);
    check_cancel(cancel)?;
    // Keep the passes separate so a large MAD calculation remains cooperatively cancellable.
    for chunk in samples.chunks_mut(NORMALIZATION_CHUNK_SIZE) {
        check_cancel(cancel)?;
        for value in chunk {
            *value = (*value - median).abs();
        }
    }
    let mad = median_f32_mut(samples);
    check_cancel(cancel)?;
    Ok(MedianMad { median, mad })
}

#[cfg(test)]
mod tests;
