//! Fitting one frame's scale against another's when both are noisy.
//!
//! Ordinary least squares assumes the x-axis is exact, which biases the slope toward zero when it
//! is not — and here both axes are sky measurements carrying the same kind of error. Deming
//! regression takes each side's noise variance and solves for the slope that accounts for both.
//! Inliers are selected first against a robust median/MAD window, so a satellite trail or a
//! mis-registered corner cannot drag the fit.

use common::CancelToken;

use crate::math::statistics::{MedianMad, mad_to_sigma};
use crate::stacking::combine::error::Error;
use crate::stacking::combine::error::check_cancel;
use crate::stacking::combine::normalization::{NORMALIZATION_CHUNK_SIZE, cancellable_median_mad};

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

    fn deming_gain(self, frame_noise_variance: f64, reference_noise_variance: f64) -> f32 {
        if self.count < 2 || self.covariance <= f64::EPSILON {
            return 1.0;
        }
        let noise_ratio =
            if frame_noise_variance > f64::EPSILON && reference_noise_variance > f64::EPSILON {
                reference_noise_variance / frame_noise_variance
            } else {
                1.0
            };
        let delta = self.reference_variance - noise_ratio * self.frame_variance;
        let root = (delta * delta + 4.0 * noise_ratio * self.covariance * self.covariance).sqrt();
        let gain = if delta >= 0.0 {
            (delta + root) / (2.0 * self.covariance)
        } else {
            2.0 * noise_ratio * self.covariance / (root - delta)
        };
        if gain.is_finite() && gain > f64::EPSILON {
            gain as f32
        } else {
            1.0
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ResidualWindow {
    gain: f32,
    offset: f32,
    center: f32,
    radius: f32,
}

pub(super) fn paired_photometric_gain(
    frame: &[f32],
    reference: &[f32],
    reference_stats: MedianMad,
    frame_noise_variance: f64,
    reference_noise_variance: f64,
    cancel: &CancelToken,
) -> Result<f32, Error> {
    let mut scratch = frame.to_vec();
    let frame_stats = cancellable_median_mad(&mut scratch, cancel)?;
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
    let residual_stats = cancellable_median_mad(&mut scratch, cancel)?;
    if residual_stats.mad <= f32::EPSILON {
        return Ok(gain);
    }
    let window = ResidualWindow {
        gain,
        offset,
        center: residual_stats.median,
        radius: 4.0 * mad_to_sigma(residual_stats.mad),
    };
    Ok(
        PairedMoments::from_inliers(frame, reference, window, cancel)?
            .deming_gain(frame_noise_variance, reference_noise_variance),
    )
}

pub(super) fn sample_stats(samples: &[f32], cancel: &CancelToken) -> Result<MedianMad, Error> {
    let mut scratch = samples.to_vec();
    cancellable_median_mad(&mut scratch, cancel)
}
