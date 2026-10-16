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

use crate::combine::reducer::pixel_samples::NoiseColumns;
use crate::combine::rejection::gesd_config::GesdConfig;
use crate::combine::rejection::linear_fit_clip_config::LinearFitClipConfig;
use crate::combine::rejection::pass::{Narrowing, Pass, Proposal};
use crate::combine::rejection::rejection_scale::RejectionScale;
use crate::combine::rejection::scratch_buffers::{MethodScratch, ScratchBuffers};
use crate::combine::rejection::sigma_bounds::SigmaBounds;
use crate::combine::rejection::sigma_clip_config::SigmaClipConfig;
use crate::combine::rejection::trim_config::TrimConfig;
use crate::combine::rejection::winsorized_clip_config::WinsorizedClipConfig;
use crate::config_value::ConfigValue;
use crate::error::InvalidConfigField;

/// An iteration cap must leave at least one pass to run. Shared by the methods that iterate.
fn validate_max_passes(max_iterations: u32) -> Result<(), InvalidConfigField> {
    ConfigValue::new("max_iterations", max_iterations).must_be("at least 1", max_iterations >= 1)
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
        Self::SigmaClip(SigmaClipConfig::default())
    }
}

impl Rejection {
    /// Sigma clipping to `sigma` either side, for at most 3 passes.
    pub const fn sigma_clip(sigma: f32) -> Self {
        Self::SigmaClip(SigmaClipConfig {
            sigma: SigmaBounds::symmetric(sigma),
            ..SigmaClipConfig::DEFAULT
        })
    }

    /// Winsorized sigma clipping to `sigma` either side.
    pub const fn winsorized(sigma: f32) -> Self {
        Self::Winsorized(WinsorizedClipConfig::new(SigmaBounds::symmetric(sigma)))
    }

    /// Linear fit clipping to `sigma` either side, for at most 4 passes.
    pub const fn linear_fit(sigma: f32) -> Self {
        Self::LinearFit(LinearFitClipConfig {
            sigma: SigmaBounds::symmetric(sigma),
            ..LinearFitClipConfig::DEFAULT
        })
    }

    /// Create a trim of the same share from each end.
    pub const fn trim(percent: f32) -> Self {
        Self::Trim(TrimConfig::new(percent, percent))
    }

    /// GESD at its default significance and test count.
    pub const fn gesd() -> Self {
        Self::Gesd(GesdConfig::DEFAULT)
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

    /// Whether the rejection holds each sample to its own CCD noise model.
    pub(crate) const fn uses_ccd_model(&self) -> bool {
        matches!(
            self,
            Self::SigmaClip(SigmaClipConfig {
                scale: RejectionScale::CcdModel,
                ..
            })
        )
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
            Self::LinearFit(config) => config.max_iterations as usize,
            Self::Winsorized(_) | Self::Trim(_) | Self::Gesd(_) => 1,
        }
    }

    /// Whether a pass that rejects nothing ends the method. Linear fit's first pass is only the
    /// robust start for the fits: an outlier the median clip keeps can still be off the fitted
    /// line.
    const fn settles(&self, index: usize) -> bool {
        !matches!(self, Self::LinearFit(_)) || index > 0
    }

    fn narrow(&self, pass: &Pass<'_>, scratch: &mut MethodScratch) -> Narrowing {
        match *self {
            Self::None => Narrowing::Settled,
            Self::SigmaClip(config) => config.narrow(pass, scratch),
            Self::Winsorized(config) => config.narrow(pass, scratch),
            Self::LinearFit(config) => config.narrow(pass, scratch),
            Self::Trim(config) => config.narrow(pass, scratch),
            Self::Gesd(config) => config.narrow(pass, scratch),
        }
    }

    /// The window of `scratch`'s sorted samples that survives. The floor under every measured σ is
    /// the noise models' σ at the pass's centre, from `noise`.
    ///
    /// Passes run until one rejects nothing, or until the method's cap. When a pass proposes fewer
    /// than `min_survivors` samples, the driver keeps the `min_survivors` samples of the window
    /// before it that sit nearest the pass's centre, and stops: a pixel never loses every sample,
    /// and the samples it keeps are the ones the method trusted most. A pass with a band for each
    /// sample reorders the window so its survivors stay one ascending run.
    pub(crate) fn surviving_window(
        &self,
        scratch: &mut ScratchBuffers,
        noise: Option<NoiseColumns<'_>>,
        min_survivors: usize,
    ) -> Range<usize> {
        debug_assert!(min_survivors >= 1);
        let ScratchBuffers {
            sorted, methods, ..
        } = scratch;
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
            let proposal = match self.narrow(&pass, methods) {
                Narrowing::Settled => break,
                Narrowing::Window(proposal) => proposal,
                Narrowing::PerSample(band) => {
                    let noise = noise.expect("a per-sample band reads the noise models");
                    let keep =
                        |value: f32, position: u32| band.keeps(&noise, position as usize, value);
                    if sorted.classify(window.clone(), keep) < min_survivors {
                        window = sorted.nearest(window, band.centre, min_survivors);
                        break;
                    }
                    Proposal {
                        window: sorted.partition(window.clone(), band.centre),
                        centre: band.centre,
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
                window = sorted.nearest(window, proposal.centre, min_survivors);
                break;
            }
            window = proposal.window;
        }
        window
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
