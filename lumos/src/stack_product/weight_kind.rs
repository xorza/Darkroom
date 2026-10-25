//! [`WeightKind`]: what the frame weights a stack product's weight plane sums were.

use crate::io::fits::lumos_extension::WeightUnits;

/// What the frame weights that a [`StackProduct::weight`](crate::StackProduct::weight) plane sums
/// were, as the weighting stated them: what the plane means, and the units it is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeightKind {
    /// Unit weights: the plane counts the frames that formed a pixel, each a drizzled frame's
    /// share of its drops, and carries no units.
    Count,
    /// Each frame's inverse noise variance: the plane is an inverse variance, in the image's units
    /// to the power −2.
    InverseVariance,
    /// The caller's relative weights: the plane is relative too, and carries no units.
    Relative,
}

impl WeightKind {
    /// The units the weight plane is in, which set the scaling a FITS file stores it at.
    pub(crate) const fn units(self) -> WeightUnits {
        match self {
            Self::Count | Self::Relative => WeightUnits::Unitless,
            Self::InverseVariance => WeightUnits::InverseSquare,
        }
    }
}
