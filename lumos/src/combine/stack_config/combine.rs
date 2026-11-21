//! [`Combine`]: how the samples at one pixel become one value, and its presets.

use crate::combine::rejection::Rejection;
use crate::combine::stack_config::small_n::SmallN;
use crate::stack_product::quality_planes::QualityPlanes;

/// How the samples at one pixel become one value.
///
/// A small-sample fallback exists only beside a rejection, whose statistics it stands in for, and
/// it never rejects itself: both by the variants. Refused: a mean with `Rejection::None`, a "no
/// rejection" spelled inside the rejection; a fallback typed as a combine method, which admits a
/// rejecting one. The constructors are the presets; the normalization and the weighting belong to
/// the role of the frames, which [`StackConfig`](crate::StackConfig)'s presets carry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Combine {
    /// The median of the samples: implicit outlier rejection, weighted by nothing.
    Median,
    /// The weighted mean of every sample.
    Mean,
    /// The weighted mean of the samples `rejection` keeps, or, at a pixel fewer than its `small_n`
    /// floor of frames reached, its fallback.
    RejectedMean {
        rejection: Rejection,
        small_n: Option<SmallN>,
    },
}

impl Combine {
    /// The weighted mean of what `rejection` keeps, below the frames its statistics need falling
    /// back as [`Rejection::small_n`] says.
    pub const fn rejected(rejection: Rejection) -> Self {
        Self::RejectedMean {
            rejection,
            small_n: rejection.small_n(),
        }
    }

    /// Sigma-clipped mean, with the median below the frames sigma statistics need.
    pub const fn sigma_clip(sigma: f32) -> Self {
        Self::rejected(Rejection::sigma_clip(sigma))
    }

    /// Winsorized sigma clipping, with no median fallback: it clips once, about a Huber estimate of
    /// every sample. On ten clean samples at k = 3 it rejects 0.12% under the frames' noise floor
    /// and 1.5% with none, against the Gaussian tail share of 0.27%.
    pub const fn winsorized(sigma: f32) -> Self {
        Self::rejected(Rejection::winsorized(sigma))
    }

    /// Linear fit clipping (good for sky gradients), with the median below the frames its fit
    /// needs.
    pub const fn linear_fit(sigma: f32) -> Self {
        Self::rejected(Rejection::linear_fit(sigma))
    }

    /// A trimmed mean, dropping `percent` of the samples from each end. A trim measures no spread,
    /// so a small stack needs no median fallback.
    pub const fn trim(percent: f32) -> Self {
        Self::rejected(Rejection::trim(percent))
    }

    /// Validation-constrained automatic GESD with a median fallback below its supported sample
    /// size.
    pub const fn gesd() -> Self {
        Self::rejected(Rejection::gesd())
    }

    /// The planes of `requested` this combine can produce, so the request reaching the reducer is
    /// exactly what it will write: inverse variance and dispersion belong to a weighted mean, and a
    /// median is not a linear combination.
    pub(crate) const fn planes(self, requested: QualityPlanes) -> QualityPlanes {
        let weighted_mean = !matches!(self, Self::Median);
        QualityPlanes {
            inverse_variance: requested.inverse_variance && weighted_mean,
            dispersion: requested.dispersion && weighted_mean,
            ..requested
        }
    }
}
