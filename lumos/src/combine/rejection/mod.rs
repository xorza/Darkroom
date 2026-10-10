//! Pixel rejection before the mean.
//!
//! [`Rejection`] is the enum a caller picks from. Every method works the same way: the pixel's
//! samples are sorted once, and each pass of the method narrows a window of them. Sigma clip,
//! winsorized, GESD and trim reject only from the ends by their nature, and linear fit is made to.
//! The driver here owns what the methods share: the sort, the loop over passes, the noise floor
//! under every σ, and the rule that keeps at least `min_survivors` samples.

pub(crate) mod gesd_config;
pub(crate) mod linear_fit_clip_config;
pub(crate) mod normal_scores;
pub(crate) mod pass;
pub(crate) mod rejection_scale;
pub(crate) mod scratch_buffers;
pub(crate) mod sigma_bounds;
pub(crate) mod sigma_clip_config;
pub(crate) mod sorted_samples;
pub(crate) mod trim_config;
pub(crate) mod winsorized_clip_config;

use std::ops::Range;

use crate::combine::cache::sample::{CombinedSample, PixelSamples};
use crate::combine::cache::sample_noise::NoiseColumns;
use crate::combine::rejection::gesd_config::GesdConfig;
use crate::combine::rejection::linear_fit_clip_config::LinearFitClipConfig;
use crate::combine::rejection::pass::{Narrowing, Pass, Proposal};
use crate::combine::rejection::scratch_buffers::{MethodScratch, ScratchBuffers};
use crate::combine::rejection::sigma_clip_config::SigmaClipConfig;
use crate::combine::rejection::sorted_samples::SortedSamples;
use crate::combine::rejection::trim_config::TrimConfig;
use crate::combine::rejection::winsorized_clip_config::WinsorizedClipConfig;
use crate::error::InvalidConfigField;
use crate::math::sum;

/// An iteration cap must leave at least one pass to run. Shared by the methods that iterate.
fn validate_max_iterations(max_iterations: u32) -> Result<(), InvalidConfigField> {
    InvalidConfigField::check(
        max_iterations >= 1,
        "max_iterations",
        "at least 1",
        f64::from(max_iterations),
    )
}

/// Pixel rejection algorithm applied before combining.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rejection {
    /// No rejection.
    None,
    /// Iterative sigma clipping about the median, symmetric or asymmetric.
    SigmaClip(SigmaClipConfig),
    /// Sigma clipping about a Huber estimate of the centre and σ.
    Winsorized(WinsorizedClipConfig),
    /// Sigma clipping about a line fitted through the sorted samples on their normal scores.
    LinearFit(LinearFitClipConfig),
    /// Drop a fixed share from each end: a trimmed mean.
    Trim(TrimConfig),
    /// Generalized ESD test (best for large stacks >50 frames).
    Gesd(GesdConfig),
}

impl Default for Rejection {
    fn default() -> Self {
        Self::SigmaClip(SigmaClipConfig::new(2.5, 3))
    }
}

impl Rejection {
    /// Create sigma clipping with default iterations.
    pub const fn sigma_clip(sigma: f32) -> Self {
        Self::SigmaClip(SigmaClipConfig::new(sigma, 3))
    }

    /// Create asymmetric sigma clipping.
    pub const fn sigma_clip_asymmetric(sigma_low: f32, sigma_high: f32) -> Self {
        Self::SigmaClip(SigmaClipConfig::new_asymmetric(sigma_low, sigma_high, 3))
    }

    /// Create winsorized sigma clipping.
    pub const fn winsorized(sigma: f32) -> Self {
        Self::Winsorized(WinsorizedClipConfig::new(sigma))
    }

    /// Create linear fit clipping with symmetric thresholds.
    pub const fn linear_fit(sigma: f32) -> Self {
        Self::LinearFit(LinearFitClipConfig::new(sigma, sigma, 3))
    }

    /// Create a trim of the same share from each end.
    pub const fn trim(percent: f32) -> Self {
        Self::Trim(TrimConfig::new(percent, percent))
    }

    /// Create GESD with default alpha.
    pub fn gesd() -> Self {
        Self::Gesd(GesdConfig::default())
    }

    /// Validate the held configuration, if any.
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        match self {
            Self::None => Ok(()),
            Self::SigmaClip(config) => config.validate(),
            Self::Winsorized(config) => config.validate(),
            Self::LinearFit(config) => config.validate(),
            Self::Trim(config) => config.validate(),
            Self::Gesd(config) => config.validate(),
        }
    }

    /// What a clipped pixel's dispersion is multiplied by to read the frames' full scatter: the
    /// reciprocal of a unit normal's variance truncated to the method's band, for the methods that
    /// clip in a band of σ, and 1 for the others, whose cut is not one in σ.
    pub(crate) fn dispersion_correction(&self) -> f32 {
        let bounds = match self {
            Self::SigmaClip(config) => config.sigma,
            Self::Winsorized(config) => config.sigma,
            Self::LinearFit(config) => config.sigma,
            Self::None | Self::Trim(_) | Self::Gesd(_) => return 1.0,
        };
        (1.0 / bounds.truncated_variance()) as f32
    }

    /// Whether the method measures a spread, and so needs the frames' noise as its floor.
    pub(crate) const fn measures_spread(&self) -> bool {
        !matches!(self, Self::None | Self::Trim(_))
    }

    /// The most passes the method runs.
    const fn passes(&self) -> usize {
        match self {
            Self::None => 0,
            Self::SigmaClip(config) => config.max_iterations as usize,
            Self::LinearFit(config) => config.passes(),
            Self::Winsorized(_) | Self::Trim(_) | Self::Gesd(_) => 1,
        }
    }

    /// Whether a pass that rejects nothing ends the method. Linear fit's first pass is only the
    /// robust start for the fits: an outlier the median clip keeps can still be off the fitted
    /// line.
    const fn settles(&self, index: usize) -> bool {
        !matches!(self, Self::LinearFit(_)) || index > 0
    }

    fn narrow(&self, pass: &Pass<'_>, scratch: &mut MethodScratch) -> Option<Narrowing> {
        Some(match self {
            Self::None => return None,
            Self::SigmaClip(config) => config.narrow(pass, &mut scratch.scores),
            Self::Winsorized(config) => {
                Narrowing::Window(config.narrow(pass, &mut scratch.clamped))
            }
            Self::LinearFit(config) => Narrowing::Window(config.narrow(pass, &mut scratch.scores)),
            Self::Trim(config) => Narrowing::Window(config.narrow(pass)),
            Self::Gesd(config) => Narrowing::Window(config.narrow(pass, &mut scratch.gesd)?),
        })
    }

    /// The window of `sorted` that survives. The floor under every measured σ is the noise
    /// models' σ at the pass's centre, from `noise`.
    ///
    /// Passes run until one rejects nothing, or until the method's cap. When a pass proposes fewer
    /// than `min_survivors` samples, the driver keeps the `min_survivors` samples of the window
    /// before it that sit nearest the pass's centre, and stops: a pixel never loses every sample,
    /// and the samples it keeps are the ones the method trusted most. A pass with a band for each
    /// sample reorders the window so its survivors stay one ascending run.
    pub(crate) fn surviving_window(
        &self,
        sorted: &mut SortedSamples,
        noise: Option<NoiseColumns<'_>>,
        min_survivors: usize,
        scratch: &mut MethodScratch,
    ) -> Range<usize> {
        debug_assert!(min_survivors >= 1);
        let noise = noise.filter(|_| self.measures_spread());
        let mut window = 0..sorted.values().len();
        for index in 0..self.passes() {
            if window.len() <= min_survivors {
                break;
            }
            let pass = Pass {
                sorted: sorted.values(),
                positions: sorted.positions(),
                window: window.clone(),
                index,
                min_survivors,
                noise,
            };
            let proposal = match self.narrow(&pass, scratch) {
                None => break,
                Some(Narrowing::Window(proposal)) => proposal,
                Some(Narrowing::PerSample { centre, bounds }) => {
                    let noise = noise.expect("a per-sample band reads the noise models");
                    let keep = |value: f32, position: u32| {
                        noise.within(position as usize, value, centre, bounds)
                    };
                    if sorted.count_kept(window.clone(), keep) < min_survivors {
                        window = nearest(sorted.values(), window, centre, min_survivors);
                        break;
                    }
                    Proposal {
                        window: sorted.partition(window.clone(), centre, keep),
                        centre,
                    }
                }
            };
            debug_assert!(
                window.start <= proposal.window.start && proposal.window.end <= window.end,
                "a pass widened its window: {window:?} to {:?}",
                proposal.window
            );
            if proposal.window == window && self.settles(index) {
                break;
            }
            if proposal.window.len() < min_survivors {
                window = nearest(sorted.values(), window, proposal.centre, min_survivors);
                break;
            }
            window = proposal.window;
        }
        window
    }

    /// Reject outliers, then reduce the survivors to their weighted mean.
    ///
    /// The one reduction entry point. `measure_quality` asks for the survivors' effective weight as
    /// well, which is a second pass over the samples.
    ///
    /// Only the mean is weighted; the rejection decides survivors from the values alone. That
    /// matches what `ImageIntegration`, Siril and DSS do: rejection asks which samples disagree
    /// with the others, which is a question about the normalized values, not about how much each
    /// frame is trusted. It also keeps GESD a valid test: its critical values come from the
    /// t-distribution for `n` iid observations, and there is no weighted form of them.
    pub(crate) fn combine_mean(
        &self,
        samples: PixelSamples<'_>,
        min_survivors: usize,
        scratch: &mut ScratchBuffers,
        measure_quality: bool,
    ) -> CombinedSample {
        let PixelSamples {
            values,
            weights,
            noise,
            ..
        } = samples;
        debug_assert_eq!(values.len(), weights.len());
        if matches!(self, Self::None) || values.len() <= min_survivors {
            scratch.survivors = None;
            let value = sum::weighted_mean_f32(values, weights);
            return if measure_quality {
                CombinedSample::from_survivors(value, values, weights, 0..values.len(), noise)
            } else {
                CombinedSample::value_only(value)
            };
        }

        scratch.sorted.fill(values);
        let window = self.surviving_window(
            &mut scratch.sorted,
            noise,
            min_survivors,
            &mut scratch.methods,
        );
        let positions = &scratch.sorted.positions()[window.clone()];
        scratch.weights.clear();
        scratch
            .weights
            .extend(positions.iter().map(|&position| weights[position as usize]));
        let value =
            sum::weighted_mean_f32(&scratch.sorted.values()[window.clone()], &scratch.weights);
        let sample = if measure_quality {
            CombinedSample::from_survivors(
                value,
                values,
                weights,
                positions.iter().map(|&position| position as usize),
                noise,
            )
        } else {
            CombinedSample::value_only(value)
        };
        scratch.survivors = Some(window);
        sample
    }
}

/// The `count` samples of `window` nearest `centre`: a run of the sorted samples, found by dropping
/// the farther end until `count` remain. A tie drops the higher end, so the lower position is kept.
fn nearest(sorted: &[f32], window: Range<usize>, centre: f32, count: usize) -> Range<usize> {
    let Range { mut start, mut end } = window;
    while end - start > count {
        if (sorted[end - 1] - centre).abs() >= (sorted[start] - centre).abs() {
            end -= 1;
        } else {
            start += 1;
        }
    }
    start..end
}

#[cfg(test)]
mod tests;
