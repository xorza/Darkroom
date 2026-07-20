//! Frame normalization measurement and parameter fitting.

use arrayvec::ArrayVec;

use crate::io::astro_image::ImageDimensions;
use crate::math::statistics::{ChannelStats, mad_to_sigma, median_and_mad_f32_mut};
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
    fn from_inliers(frame: &[f32], reference: &[f32], window: ResidualWindow) -> Self {
        let mut moments = Self {
            count: 0,
            mean_frame: 0.0,
            mean_reference: 0.0,
            frame_variance: 0.0,
            reference_variance: 0.0,
            covariance: 0.0,
        };
        for (&frame_value, &reference_value) in frame.iter().zip(reference) {
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
        moments
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
) -> Result<Option<Vec<FrameNorm>>, Error> {
    if normalization == Normalization::None {
        return Ok(None);
    }

    let reference = select_reference_frame(frames.iter().map(|frame| &frame.source_stats));
    let registered = frames
        .iter()
        .any(|frame| frame.coverage.is_some() || frame.confidence.is_some());
    if !registered {
        return Ok(Some(compute_frame_norms_with_reference(
            frames.iter().map(|frame| &frame.source_stats),
            normalization,
            reference,
        )));
    }

    match measure_registered_frames(frames, dimensions, normalization)? {
        RegisteredMeasurements::GlobalNormsToFirst(mut norms) => {
            let reference_norm = norms[reference].clone();
            for (frame_index, frame_norm) in norms.iter_mut().enumerate() {
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
) -> Result<RegisteredMeasurements, Error> {
    let pixel_count = dimensions.pixel_count();
    let mut common_domain = vec![true; pixel_count];
    for frame in frames {
        if let Some(coverage) = &frame.coverage {
            for (valid, &value) in common_domain.iter_mut().zip(coverage.chunk(0, pixel_count)) {
                *valid &= value > MIN_CONTRIBUTING_COVERAGE;
            }
        }
        if let Some(confidence) = &frame.confidence {
            for (valid, &value) in common_domain
                .iter_mut()
                .zip(confidence.chunk(0, pixel_count))
            {
                *valid &= value > 0.0;
            }
        }
    }
    let sample_count = common_domain.iter().filter(|&&valid| valid).count();
    if sample_count == 0 {
        return Err(Error::NoCommonCoverage);
    }

    let photometric_indices = (normalization == Normalization::Global)
        .then(|| stratified_valid_indices(&common_domain, sample_count));
    let mut common_stats: Vec<FrameStats> = frames
        .iter()
        .map(|_| FrameStats {
            channels: ArrayVec::new(),
        })
        .collect();
    let mut global_norms_to_first = photometric_indices.as_ref().map(|_| {
        frames
            .iter()
            .map(|frame| identity_norm(frame.channels.len()))
            .collect::<Vec<_>>()
    });
    let mut samples = Vec::with_capacity(sample_count);

    for channel in 0..dimensions.channels() {
        gather_valid_samples(
            &mut samples,
            &frames[0].channels[channel],
            &common_domain,
            pixel_count,
        );
        let (reference_median, reference_mad) = median_and_mad_f32_mut(&mut samples);
        common_stats[0].channels.push(ChannelStats {
            median: reference_median,
            mad: reference_mad,
        });
        let reference_fit = photometric_indices.as_ref().map(|indices| {
            gather_indexed_samples(&frames[0].channels[channel], indices, pixel_count)
        });

        for frame_index in 1..frames.len() {
            gather_valid_samples(
                &mut samples,
                &frames[frame_index].channels[channel],
                &common_domain,
                pixel_count,
            );
            let (median, mad) = median_and_mad_f32_mut(&mut samples);
            common_stats[frame_index]
                .channels
                .push(ChannelStats { median, mad });

            if let (Some(indices), Some(reference), Some(norms)) = (
                &photometric_indices,
                &reference_fit,
                &mut global_norms_to_first,
            ) {
                let frame_samples = gather_indexed_samples(
                    &frames[frame_index].channels[channel],
                    indices,
                    pixel_count,
                );
                let gain = paired_photometric_gain(
                    &frame_samples,
                    reference,
                    source_noise_variance(&frames[frame_index], channel, indices, pixel_count),
                    source_noise_variance(&frames[0], channel, indices, pixel_count),
                );
                norms[frame_index].channels[channel] = ChannelNorm {
                    gain,
                    offset: reference_median - median * gain,
                };
            }
        }
    }

    Ok(match normalization {
        Normalization::Global => RegisteredMeasurements::GlobalNormsToFirst(
            global_norms_to_first.expect("global normalization initializes affine fits"),
        ),
        Normalization::Multiplicative => RegisteredMeasurements::CommonStats(common_stats),
        Normalization::None => unreachable!(),
    })
}

fn gather_valid_samples(
    samples: &mut Vec<f32>,
    plane: &StoredPlane,
    common_domain: &[bool],
    pixel_count: usize,
) {
    samples.clear();
    samples.extend(
        plane
            .chunk(0, pixel_count)
            .iter()
            .zip(common_domain)
            .filter_map(|(&value, &valid)| valid.then_some(value)),
    );
}

fn stratified_valid_indices(common_domain: &[bool], sample_count: usize) -> Vec<usize> {
    let retained = sample_count.min(PHOTOMETRIC_SAMPLE_LIMIT);
    let mut indices = Vec::with_capacity(retained);
    let mut valid_rank = 0;
    for (pixel, &valid) in common_domain.iter().enumerate() {
        if !valid {
            continue;
        }
        if indices.len() < retained && valid_rank == indices.len() * sample_count / retained {
            indices.push(pixel);
        }
        valid_rank += 1;
    }
    debug_assert_eq!(indices.len(), retained);
    indices
}

fn gather_indexed_samples(plane: &StoredPlane, indices: &[usize], pixel_count: usize) -> Vec<f32> {
    let pixels = plane.chunk(0, pixel_count);
    indices.iter().map(|&index| pixels[index]).collect()
}

fn source_noise_variance(
    frame: &StoredLightFrame,
    channel: usize,
    indices: &[usize],
    pixel_count: usize,
) -> f64 {
    let sigma = f64::from(mad_to_sigma(frame.source_stats.channels[channel].mad));
    let mean_inverse_confidence = frame.confidence.as_ref().map_or(1.0, |confidence| {
        let values = confidence.chunk(0, pixel_count);
        indices
            .iter()
            .map(|&index| 1.0 / f64::from(values[index]))
            .sum::<f64>()
            / indices.len() as f64
    });
    sigma * sigma * mean_inverse_confidence
}

fn paired_photometric_gain(
    frame: &[f32],
    reference: &[f32],
    frame_noise_variance: f64,
    reference_noise_variance: f64,
) -> f32 {
    let frame_stats = sample_stats(frame);
    let reference_stats = sample_stats(reference);
    let gain = if frame_stats.mad > f32::EPSILON {
        reference_stats.mad / frame_stats.mad
    } else {
        1.0
    };
    let offset = reference_stats.median - frame_stats.median * gain;
    let mut residuals: Vec<f32> = frame
        .iter()
        .zip(reference)
        .map(|(&frame_value, &reference_value)| reference_value - (frame_value * gain + offset))
        .collect();
    let (center, mad) = median_and_mad_f32_mut(&mut residuals);
    if mad <= f32::EPSILON {
        return gain;
    }
    let window = ResidualWindow {
        gain,
        offset,
        center,
        radius: 4.0 * mad_to_sigma(mad),
    };
    deming_gain(
        PairedMoments::from_inliers(frame, reference, window),
        frame_noise_variance,
        reference_noise_variance,
    )
}

fn sample_stats(samples: &[f32]) -> ChannelStats {
    let mut scratch = samples.to_vec();
    let (median, mad) = median_and_mad_f32_mut(&mut scratch);
    ChannelStats { median, mad }
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
