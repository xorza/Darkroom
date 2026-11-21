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
mod percent;
pub(crate) mod rejection_scale;
pub(crate) mod scratch_buffers;
pub(crate) mod sigma_bounds;
pub(crate) mod sigma_clip_config;
pub(crate) mod sorted_samples;
pub(crate) mod trim_config;
pub(crate) mod winsorized_clip_config;

use std::ops::Range;

use crate::combine::reducer::pixel_samples::NoiseCols;
use crate::combine::rejection::gesd_config::GesdConfig;
use crate::combine::rejection::linear_fit_clip_config::LinearFitClipConfig;
use crate::combine::rejection::pass::{Narrowing, Pass, Proposal};
use crate::combine::rejection::rejection_scale::RejectionScale;
use crate::combine::rejection::scratch_buffers::{MethodScratch, ScratchBuffers};
use crate::combine::rejection::sigma_bounds::SigmaBounds;
use crate::combine::rejection::sigma_clip_config::SigmaClipConfig;
use crate::combine::rejection::trim_config::{TrimBounds, TrimConfig};
use crate::combine::rejection::winsorized_clip_config::WinsorizedClipConfig;
use crate::combine::stack_config::small_n::SmallN;
use crate::error::InvalidConfigField;

/// The frames below which sigma clip and linear fit are too unreliable to trust and the combine
/// falls back to the median. GESD has its own stricter floor. Winsorized clips once, about an
/// estimate of every sample, and trim measures no spread, so neither has one.
const MIN_FRAMES_FOR_REJECTION: usize = 5;
const MIN_FRAMES_FOR_GESD: usize = 15;

/// Pixel rejection algorithm applied before a mean.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rejection {
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
        Self::Winsorized(WinsorizedClipConfig {
            sigma: SigmaBounds::symmetric(sigma),
        })
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
        Self::Trim(TrimConfig {
            percent: TrimBounds::symmetric(percent),
        })
    }

    /// GESD at its default significance and test count.
    pub const fn gesd() -> Self {
        Self::Gesd(GesdConfig::DEFAULT)
    }

    /// The fallback below the frames this method's statistics need: the median below 5 for sigma
    /// clip and linear fit, below 15 for GESD, none for Winsorized and trim, which clip once about
    /// every sample or measure no spread.
    pub const fn small_n(&self) -> Option<SmallN> {
        match self {
            Self::SigmaClip(_) | Self::LinearFit(_) => {
                Some(SmallN::median_below(MIN_FRAMES_FOR_REJECTION))
            }
            Self::Gesd(_) => Some(SmallN::median_below(MIN_FRAMES_FOR_GESD)),
            Self::Winsorized(_) | Self::Trim(_) => None,
        }
    }

    /// Validate the held configuration.
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        match self {
            Self::SigmaClip(config) => config.validate(),
            Self::Winsorized(config) => config.validate(),
            Self::LinearFit(config) => config.validate(),
            Self::Trim(config) => config.validate(),
            Self::Gesd(config) => config.validate(),
        }
    }

    /// Whether the rejection holds each sample to its own CCD noise model.
    pub(super) const fn uses_ccd_model(&self) -> bool {
        matches!(
            self,
            Self::SigmaClip(SigmaClipConfig {
                scale: RejectionScale::CcdModel,
                ..
            })
        )
    }

    /// The variance of a unit normal truncated to the method's band, `tv`, for the methods that
    /// clip in a band of σ — sigma clip, winsorized, linear fit; `None` for trim and GESD, whose
    /// cut is not one in σ.
    ///
    /// What choosing the survivors from the data costs: a mean of samples clipped at ±k about a
    /// centre the data set has the asymptotic variance `σ²/(n·Z·tv)`, `Z` the band's mass (Huber,
    /// *Robust Statistics*, §3.2, `E[ψ²]/E[ψ′]²` for `ψ(x) = x·1{|x| ≤ k}`), where the same
    /// survivors taken as fixed have `σ²/(n·Z)`: `tv` times less.
    pub(super) fn band_variance(&self) -> Option<f64> {
        let bounds = match self {
            Self::SigmaClip(config) => config.sigma,
            Self::Winsorized(config) => config.sigma,
            Self::LinearFit(config) => config.sigma,
            Self::Trim(_) | Self::Gesd(_) => return None,
        };
        Some(bounds.truncated_variance())
    }

    /// Whether the method measures a spread, and so needs the frames' noise as its floor.
    pub(super) const fn measures_spread(&self) -> bool {
        !matches!(self, Self::Trim(_))
    }

    /// The most passes the method runs.
    const fn passes(&self) -> usize {
        match self {
            Self::SigmaClip(config) => config.max_iterations as usize,
            Self::LinearFit(config) => config.max_iterations as usize,
            Self::Winsorized(_) | Self::Trim(_) | Self::Gesd(_) => 1,
        }
    }

    /// Whether a pass that rejects nothing ends the method. Linear fit's first pass is only the
    /// robust start for the fits: an outlier the median clip keeps can still be off the fitted
    /// line.
    const fn settles(&self, idx: usize) -> bool {
        !matches!(self, Self::LinearFit(_)) || idx > 0
    }

    fn narrow(&self, pass: &Pass<'_>, scratch: &mut MethodScratch) -> Narrowing {
        match *self {
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
    pub(super) fn surviving_window(
        &self,
        scratch: &mut ScratchBuffers,
        noise: Option<NoiseCols<'_>>,
        min_survivors: usize,
    ) -> Range<usize> {
        debug_assert!(min_survivors >= 1);
        let ScratchBuffers {
            sorted, methods, ..
        } = scratch;
        let noise = noise.filter(|_| self.measures_spread());
        let mut window = 0..sorted.values().len();
        for idx in 0..self.passes() {
            if window.len() <= min_survivors {
                break;
            }
            let pass = Pass {
                sorted: sorted.values(),
                poss: sorted.poss(),
                window: window.clone(),
                idx,
                min_survivors,
                noise,
            };
            let proposal = match self.narrow(&pass, methods) {
                Narrowing::Settled => break,
                Narrowing::Window(proposal) => proposal,
                Narrowing::PerSample(band) => {
                    let noise = noise.expect("a per-sample band reads the noise models");
                    let keep = |value: f32, pos: u32| band.keeps(&noise, pos as usize, value);
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
            if proposal.window == window && self.settles(idx) {
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
