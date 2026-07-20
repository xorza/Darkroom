//! Frame normalization measurement and parameter fitting.

use arrayvec::ArrayVec;
use common::CancelToken;
use rayon::prelude::*;

use crate::io::astro_image::ImageDimensions;
use crate::math::statistics::{ChannelStats, mad_to_sigma, median_f32_mut};
use crate::stacking::combine::MIN_CONTRIBUTING_COVERAGE;
use crate::stacking::combine::config::Normalization;
use crate::stacking::combine::error::Error;
use crate::stacking::frame_store::{FrameStats, StoredLightFrame, StoredPlane};

/// Per-channel affine normalization applied as `normalized = raw * gain + offset`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChannelNorm {
    pub(crate) gain: f32,
    pub(crate) offset: f32,
}

impl ChannelNorm {
    pub(crate) const IDENTITY: Self = Self {
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
struct CommonDomain {
    valid: Vec<bool>,
    sample_count: usize,
}

#[derive(Debug)]
struct ReferenceFit {
    samples: Vec<f32>,
    stats: ChannelStats,
    noise_variance: f64,
}

#[derive(Debug, Clone, Copy)]
struct PairedMoments {
    count: usize,
    mean_frame: f64,
    mean_reference: f64,
    frame_variance: f64,
    reference_variance: f64,
    covariance: f64,
}

impl PairedMoments {
    fn from_inliers(
        frame: &[f32],
        reference: &[f32],
        window: ResidualWindow,
        cancel: &CancelToken,
    ) -> Result<Self, Error> {
        let mut moments = Self {
            count: 0,
            mean_frame: 0.0,
            mean_reference: 0.0,
            frame_variance: 0.0,
            reference_variance: 0.0,
            covariance: 0.0,
        };
        for (frame_chunk, reference_chunk) in frame
            .chunks(NORMALIZATION_CHUNK_SIZE)
            .zip(reference.chunks(NORMALIZATION_CHUNK_SIZE))
        {
            check_cancel(cancel)?;
            for (&frame_value, &reference_value) in frame_chunk.iter().zip(reference_chunk) {
                let residual = reference_value - (frame_value * window.gain + window.offset);
                if (residual - window.center).abs() > window.radius {
                    continue;
                }
                moments.count += 1;
                let count = moments.count as f64;
                let frame_value = f64::from(frame_value);
                let reference_value = f64::from(reference_value);
                let frame_delta = frame_value - moments.mean_frame;
                moments.mean_frame += frame_delta / count;
                let reference_delta = reference_value - moments.mean_reference;
                moments.mean_reference += reference_delta / count;
                moments.frame_variance += frame_delta * (frame_value - moments.mean_frame);
                moments.reference_variance +=
                    reference_delta * (reference_value - moments.mean_reference);
                moments.covariance += frame_delta * (reference_value - moments.mean_reference);
            }
        }
        Ok(moments)
    }
}

#[derive(Debug, Clone, Copy)]
struct ResidualWindow {
    gain: f32,
    offset: f32,
    center: f32,
    radius: f32,
}

const PHOTOMETRIC_SAMPLE_LIMIT: usize = 65_536;
const NORMALIZATION_CHUNK_SIZE: usize = 16_384;

pub(crate) fn compute_frame_norms(
    stats: &[FrameStats],
    normalization: Normalization,
) -> Option<Vec<FrameNorm>> {
    if normalization == Normalization::None {
        return None;
    }
    let reference = select_reference_frame(stats.iter());
    Some(compute_frame_norms_with_reference(
        stats.iter(),
        normalization,
        reference,
    ))
}

pub(crate) fn compute_light_frame_norms(
    frames: &[StoredLightFrame],
    dimensions: ImageDimensions,
    normalization: Normalization,
    cancel: &CancelToken,
) -> Result<Option<Vec<FrameNorm>>, Error> {
    if normalization == Normalization::None {
        return Ok(None);
    }
    check_cancel(cancel)?;

    let reference = select_reference_frame(frames.iter().map(|frame| &frame.source_stats));
    let registered = frames
        .iter()
        .any(|frame| frame.coverage.is_some() || frame.confidence.is_some());
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
        let ChannelStats {
            median: reference_median,
            mad: reference_mad,
        } = stats[reference].channels[channel];

        for (frame_index, frame_stats) in stats.iter().enumerate() {
            if frame_index == reference {
                continue;
            }
            let ChannelStats {
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
    frames: &[StoredLightFrame],
    dimensions: ImageDimensions,
    normalization: Normalization,
    cancel: &CancelToken,
) -> Result<RegisteredMeasurements, Error> {
    let pixel_count = dimensions.pixel_count();
    let common_domain = build_common_domain(frames, pixel_count, cancel)?;
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

fn build_common_domain(
    frames: &[StoredLightFrame],
    pixel_count: usize,
    cancel: &CancelToken,
) -> Result<CommonDomain, Error> {
    let mut common_domain = vec![true; pixel_count];
    for frame in frames {
        check_cancel(cancel)?;
        if let Some(coverage) = &frame.coverage {
            intersect_domain(
                &mut common_domain,
                coverage,
                pixel_count,
                |value| value > MIN_CONTRIBUTING_COVERAGE,
                cancel,
            )?;
        }
        if let Some(confidence) = &frame.confidence {
            intersect_domain(
                &mut common_domain,
                confidence,
                pixel_count,
                |value| value > 0.0,
                cancel,
            )?;
        }
    }
    let mut sample_count = 0;
    for chunk in common_domain.chunks(NORMALIZATION_CHUNK_SIZE) {
        check_cancel(cancel)?;
        sample_count += chunk.iter().filter(|&&valid| valid).count();
    }
    if sample_count == 0 {
        return Err(Error::NoCommonCoverage);
    }
    Ok(CommonDomain {
        valid: common_domain,
        sample_count,
    })
}

fn intersect_domain(
    common_domain: &mut [bool],
    plane: &StoredPlane,
    pixel_count: usize,
    is_valid: impl Fn(f32) -> bool,
    cancel: &CancelToken,
) -> Result<(), Error> {
    for (domain_chunk, value_chunk) in common_domain
        .chunks_mut(NORMALIZATION_CHUNK_SIZE)
        .zip(plane.chunk(0, pixel_count).chunks(NORMALIZATION_CHUNK_SIZE))
    {
        check_cancel(cancel)?;
        for (valid, &value) in domain_chunk.iter_mut().zip(value_chunk) {
            *valid &= is_valid(value);
        }
    }
    Ok(())
}

fn measure_common_stats(
    frames: &[StoredLightFrame],
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
                channel_stats(samples, cancel)
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
    frames: &[StoredLightFrame],
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
    common_domain: &[bool],
    pixel_count: usize,
    cancel: &CancelToken,
) -> Result<(), Error> {
    samples.clear();
    for (value_chunk, valid_chunk) in plane
        .chunk(0, pixel_count)
        .chunks(NORMALIZATION_CHUNK_SIZE)
        .zip(common_domain.chunks(NORMALIZATION_CHUNK_SIZE))
    {
        check_cancel(cancel)?;
        samples.extend(
            value_chunk
                .iter()
                .zip(valid_chunk)
                .filter_map(|(&value, &valid)| valid.then_some(value)),
        );
    }
    Ok(())
}

fn stratified_valid_indices(
    common_domain: &[bool],
    sample_count: usize,
    cancel: &CancelToken,
) -> Result<Vec<usize>, Error> {
    let retained = sample_count.min(PHOTOMETRIC_SAMPLE_LIMIT);
    let mut indices = Vec::with_capacity(retained);
    let mut valid_rank = 0;
    for (pixel, &valid) in common_domain.iter().enumerate() {
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
    frame: &StoredLightFrame,
    channel: usize,
    indices: &[usize],
    pixel_count: usize,
    cancel: &CancelToken,
) -> Result<f64, Error> {
    let sigma = f64::from(mad_to_sigma(frame.source_stats.channels[channel].mad));
    let Some(confidence) = &frame.confidence else {
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

fn check_cancel(cancel: &CancelToken) -> Result<(), Error> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(())
}

fn channel_stats(samples: &mut [f32], cancel: &CancelToken) -> Result<ChannelStats, Error> {
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
    Ok(ChannelStats { median, mad })
}

fn paired_photometric_gain(
    frame: &[f32],
    reference: &[f32],
    reference_stats: ChannelStats,
    frame_noise_variance: f64,
    reference_noise_variance: f64,
    cancel: &CancelToken,
) -> Result<f32, Error> {
    let mut scratch = frame.to_vec();
    let frame_stats = channel_stats(&mut scratch, cancel)?;
    let gain = if frame_stats.mad > f32::EPSILON {
        reference_stats.mad / frame_stats.mad
    } else {
        1.0
    };
    let offset = reference_stats.median - frame_stats.median * gain;
    scratch.clear();
    for (frame_chunk, reference_chunk) in frame
        .chunks(NORMALIZATION_CHUNK_SIZE)
        .zip(reference.chunks(NORMALIZATION_CHUNK_SIZE))
    {
        check_cancel(cancel)?;
        scratch.extend(frame_chunk.iter().zip(reference_chunk).map(
            |(&frame_value, &reference_value)| reference_value - (frame_value * gain + offset),
        ));
    }
    let residual_stats = channel_stats(&mut scratch, cancel)?;
    if residual_stats.mad <= f32::EPSILON {
        return Ok(gain);
    }
    let window = ResidualWindow {
        gain,
        offset,
        center: residual_stats.median,
        radius: 4.0 * mad_to_sigma(residual_stats.mad),
    };
    Ok(deming_gain(
        PairedMoments::from_inliers(frame, reference, window, cancel)?,
        frame_noise_variance,
        reference_noise_variance,
    ))
}

fn sample_stats(samples: &[f32], cancel: &CancelToken) -> Result<ChannelStats, Error> {
    let mut scratch = samples.to_vec();
    channel_stats(&mut scratch, cancel)
}

fn deming_gain(
    moments: PairedMoments,
    frame_noise_variance: f64,
    reference_noise_variance: f64,
) -> f32 {
    if moments.count < 2 || moments.covariance <= f64::EPSILON {
        return 1.0;
    }
    let noise_ratio =
        if frame_noise_variance > f64::EPSILON && reference_noise_variance > f64::EPSILON {
            reference_noise_variance / frame_noise_variance
        } else {
            1.0
        };
    let delta = moments.reference_variance - noise_ratio * moments.frame_variance;
    let root = (delta * delta + 4.0 * noise_ratio * moments.covariance * moments.covariance).sqrt();
    let gain = if delta >= 0.0 {
        (delta + root) / (2.0 * moments.covariance)
    } else {
        2.0 * noise_ratio * moments.covariance / (root - delta)
    };
    if gain.is_finite() && gain > f64::EPSILON {
        gain as f32
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests;
