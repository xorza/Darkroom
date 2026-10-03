//! [`Pass`]: what one rejection pass sees, and the [`Proposal`] it gives back.

use std::ops::Range;

use crate::combine::cache::sample_noise::NoiseColumns;
use crate::combine::rejection::sigma_bounds::SigmaBounds;
use crate::math::statistics::spread::Spread;

/// One pass over a pixel's sorted samples: the window still kept, and the noise the frames were
/// measured to have there.
#[derive(Debug)]
pub(crate) struct Pass<'a> {
    /// All of the pixel's samples, ascending. A method that needs a sample's rank among all of them
    /// reads it from here.
    pub(crate) sorted: &'a [f32],
    /// Each sorted sample's gather position, which indexes `noise`.
    pub(crate) positions: &'a [u32],
    pub(crate) window: Range<usize>,
    /// Counts from 0.
    pub(crate) index: usize,
    /// The floor under every σ the pass measures, from [`Spread::floored`].
    pub(crate) background: f32,
    pub(crate) min_survivors: usize,
    /// The samples' noise models, by gather position, when the combine gathered them.
    pub(crate) noise: Option<NoiseColumns<'a>>,
}

/// The window a pass keeps, and the centre that the survivor rule measures nearness from when the
/// window holds too few samples.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Proposal {
    pub(crate) window: Range<usize>,
    pub(crate) centre: f32,
}

impl Pass<'_> {
    /// The samples still kept, ascending.
    pub(crate) fn samples(&self) -> &[f32] {
        &self.sorted[self.window.clone()]
    }

    /// Keep the samples within `bounds` of `centre`, in units of `sigma`. On sorted samples the
    /// band is one run, so the result is a narrower window.
    pub(crate) fn keep(&self, bounds: SigmaBounds, centre: f32, sigma: f32) -> Proposal {
        debug_assert!(sigma > 0.0);
        let samples = self.samples();
        let low = centre - bounds.low * sigma;
        let high = centre + bounds.high * sigma;
        let start = self.window.start;
        Proposal {
            window: start + samples.partition_point(|&v| v < low)
                ..start + samples.partition_point(|&v| v <= high),
            centre,
        }
    }

    /// The sigma-clip step: a band about the median, in units of the floored MAD σ.
    pub(crate) fn clip_about_median(&self, bounds: SigmaBounds) -> Proposal {
        let spread = Spread::of_sorted(self.samples());
        self.keep(bounds, spread.centre, spread.floored(self.background))
    }

    /// The sigma-clip step in units of the noise model at the median: the root mean square of the
    /// kept samples' model variance there, raised to the resolution at the median.
    ///
    /// # Panics
    /// When the combine gathered no noise, which it does for every method that measures a spread.
    pub(crate) fn clip_by_model(&self, bounds: SigmaBounds) -> Proposal {
        let noise = self
            .noise
            .expect("a spread-measuring combine gathers the noise models");
        let centre = Spread::median_of_sorted(self.samples());
        let positions = &self.positions[self.window.clone()];
        let variance = positions
            .iter()
            .map(|&position| noise.variance_at(position as usize, centre))
            .sum::<f32>()
            / positions.len() as f32;
        let model = Spread {
            centre,
            sigma: variance.sqrt(),
        };
        self.keep(bounds, centre, model.floored(0.0))
    }
}
