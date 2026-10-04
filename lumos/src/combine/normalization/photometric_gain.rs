//! Fitting one frame's scale against another's when both are noisy.
//!
//! Ordinary least squares assumes the x-axis is exact, which biases the slope toward zero when it
//! is not — and here both axes are sky measurements carrying the same kind of error. Deming
//! regression takes each side's noise variance and solves for the slope that accounts for both.
//!
//! The slope is constrained by the pixels with a lever arm — stars — so the inlier window must
//! keep them. A window sized by the residual spread alone does not: that spread is sky noise,
//! and a star at 0.5 whose seed gain is 5% off sits 0.025 away, far outside a few sky σ, leaving
//! the fit on flat sky where the slope is barely determined. The window here is each pair's
//! expected residual instead — sky noise plus the seed's gain scatter times the pair's distance
//! from the median — and the seed is the median ratio over the lever-arm pairs themselves, so a
//! star is kept unless its ratio is out of line with the others' (a saturated or variable one),
//! and a satellite trail or a mis-registered corner is still cut.

use common::CancelToken;

use crate::combine::CANCEL_POLL_CHUNK;
use crate::combine::error::StackError;
use crate::io::cancelled::Cancelled;
use crate::math::statistics::spread::Spread;
use crate::math::statistics::{MedianMad, mad_to_sigma};

/// Pairs further than this many σ above their median on both sides carry the lever arm the gain
/// is read from: at 5σ the noise in one pair's ratio is under a fifth of the ratio.
const LEVER_SIGMAS: f32 = 5.0;

/// Fewest lever-arm pairs whose median ratio seeds the gain. Fewer, and the field has no stars to
/// speak of: the seed is the ratio of the sky spreads, and the window is the sky's alone.
const MIN_LEVER_PAIRS: usize = 16;

/// The inlier window's half-width, in σ of each pair's expected residual.
const WINDOW_SIGMAS: f64 = 4.0;

/// Deming fits per pair of frames, at most: the first in the seed's window, the second in the
/// window re-centred on the first's gain. A fit that returns the gain it started from ends the
/// loop early.
const FITS: usize = 2;

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
    ) -> Result<Self, StackError> {
        let mut moments = Self {
            count: 0,
            mean_frame: 0.0,
            mean_reference: 0.0,
            frame_variance: 0.0,
            reference_variance: 0.0,
            covariance: 0.0,
        };
        for (frame_chunk, reference_chunk) in frame
            .chunks(CANCEL_POLL_CHUNK)
            .zip(reference.chunks(CANCEL_POLL_CHUNK))
        {
            Cancelled::check(cancel)?;
            for (&frame_value, &reference_value) in frame_chunk.iter().zip(reference_chunk) {
                if !window.admits(frame_value, reference_value) {
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

    /// The Deming slope for the noise ratio `λ = σ²_ref / σ²_frame`, in the branch of the root
    /// that does not cancel; `None` when the inliers carry no positive covariance to fit. A ratio
    /// with a side of no measured noise is undefined, and the slope takes λ = 1: total least
    /// squares.
    fn deming_gain(self, frame_noise_variance: f64, reference_noise_variance: f64) -> Option<f32> {
        if self.count < 2 || self.covariance <= 0.0 {
            return None;
        }
        let noise_ratio = if frame_noise_variance > 0.0 && reference_noise_variance > 0.0 {
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
        (gain.is_finite() && gain > 0.0).then_some(gain as f32)
    }
}

/// The gain the window starts from, and how far one lever-arm pair's ratio scatters about it.
#[derive(Debug, Clone, Copy)]
struct Seed {
    gain: f32,
    /// 1.4826·MAD of the lever-arm ratios; 0 when there were too few to seed from.
    scatter: f32,
}

impl Seed {
    /// The median ratio `(y − m_y)/(x − m_x)` over the pairs bright on both sides, or the ratio of
    /// the sky spreads when there are too few of them.
    fn of(
        frame: &[f32],
        reference: &[f32],
        frame_stats: MedianMad,
        reference_stats: MedianMad,
        cancel: &CancelToken,
    ) -> Result<Self, StackError> {
        let frame_floor = LEVER_SIGMAS * mad_to_sigma(frame_stats.mad);
        let reference_floor = LEVER_SIGMAS * mad_to_sigma(reference_stats.mad);
        let mut ratios = Vec::new();
        for (frame_chunk, reference_chunk) in frame
            .chunks(CANCEL_POLL_CHUNK)
            .zip(reference.chunks(CANCEL_POLL_CHUNK))
        {
            Cancelled::check(cancel)?;
            ratios.extend(frame_chunk.iter().zip(reference_chunk).filter_map(
                |(&frame_value, &reference_value)| {
                    let x = frame_value - frame_stats.median;
                    let y = reference_value - reference_stats.median;
                    (x > frame_floor && y > reference_floor && x > 0.0).then(|| y / x)
                },
            ));
        }
        if ratios.len() >= MIN_LEVER_PAIRS {
            let ratio_stats = MedianMad::of_mut(&mut ratios);
            return Ok(Self {
                gain: ratio_stats.median,
                scatter: mad_to_sigma(ratio_stats.mad),
            });
        }
        // A frame whose samples tie to its own precision has no spread to compare: unit gain, which
        // the fit then moves if the pairs say otherwise.
        Ok(Self {
            gain: if frame_stats.mad > Spread::resolution(frame_stats.median) {
                reference_stats.mad / frame_stats.mad
            } else {
                1.0
            },
            scatter: 0.0,
        })
    }
}

/// Which pairs a fit admits: those whose residual from the line through the two medians at `gain`
/// lies within [`WINDOW_SIGMAS`] of its expected spread, `√(σ²_sky + (scatter · (x − m_x))²)`,
/// about the median residual.
#[derive(Debug, Clone, Copy)]
struct ResidualWindow {
    gain: f32,
    offset: f32,
    frame_median: f32,
    center: f32,
    sky_variance: f64,
    scatter: f64,
}

impl ResidualWindow {
    /// The window at `gain`, or `None` when every residual is the same and the line through the
    /// medians at `gain` is already exact.
    fn at(
        gain: f32,
        seed: Seed,
        frame: &[f32],
        reference: &[f32],
        frame_stats: MedianMad,
        reference_stats: MedianMad,
        cancel: &CancelToken,
    ) -> Result<Option<Self>, StackError> {
        let offset = reference_stats.median - frame_stats.median * gain;
        let mut residuals = Vec::with_capacity(frame.len());
        for (frame_chunk, reference_chunk) in frame
            .chunks(CANCEL_POLL_CHUNK)
            .zip(reference.chunks(CANCEL_POLL_CHUNK))
        {
            Cancelled::check(cancel)?;
            residuals.extend(frame_chunk.iter().zip(reference_chunk).map(
                |(&frame_value, &reference_value)| reference_value - (frame_value * gain + offset),
            ));
        }
        let residual_stats = MedianMad::of_mut(&mut residuals);
        if residual_stats.mad <= Spread::resolution(reference_stats.median) && seed.scatter == 0.0 {
            return Ok(None);
        }
        let sky_sigma = f64::from(mad_to_sigma(residual_stats.mad));
        Ok(Some(Self {
            gain,
            offset,
            frame_median: frame_stats.median,
            center: residual_stats.median,
            sky_variance: sky_sigma * sky_sigma,
            scatter: f64::from(seed.scatter),
        }))
    }

    fn admits(self, frame_value: f32, reference_value: f32) -> bool {
        let residual = f64::from(reference_value - (frame_value * self.gain + self.offset));
        let lever = self.scatter * f64::from(frame_value - self.frame_median);
        let spread = (self.sky_variance + lever * lever).sqrt();
        (residual - f64::from(self.center)).abs() <= WINDOW_SIGMAS * spread
    }
}

/// The gain that carries `frame` onto `reference`, from their paired samples and each side's
/// noise variance: seeded on the lever-arm pairs, then the Deming fit over the pairs its window
/// admits, re-windowed on the fitted gain.
pub(super) fn paired_photometric_gain(
    frame: &[f32],
    reference: &[f32],
    reference_stats: MedianMad,
    frame_noise_variance: f64,
    reference_noise_variance: f64,
    cancel: &CancelToken,
) -> Result<f32, StackError> {
    let frame_stats = sample_stats(frame, cancel)?;
    let seed = Seed::of(frame, reference, frame_stats, reference_stats, cancel)?;
    let mut gain = seed.gain;
    for _ in 0..FITS {
        let Some(window) = ResidualWindow::at(
            gain,
            seed,
            frame,
            reference,
            frame_stats,
            reference_stats,
            cancel,
        )?
        else {
            return Ok(gain);
        };
        // Inliers with no positive covariance cannot move the gain from where the window started.
        let Some(fitted) = PairedMoments::from_inliers(frame, reference, window, cancel)?
            .deming_gain(frame_noise_variance, reference_noise_variance)
        else {
            break;
        };
        if fitted == gain {
            break;
        }
        gain = fitted;
    }
    Ok(gain)
}

/// The median and MAD of a sample set, leaving the samples as they were.
pub(super) fn sample_stats(samples: &[f32], cancel: &CancelToken) -> Result<MedianMad, StackError> {
    Cancelled::check(cancel)?;
    Ok(MedianMad::of_mut(&mut samples.to_vec()))
}
