//! [`WeightPlane`]: a stack product's weight plane and what its frame weights were.

use crate::image::flags::quality_map::QualityMap;
use crate::io::fits::lumos_extension::WeightUnits;

/// The WHT map, `Σwᵢ` over whatever formed each pixel, beside what the frame weights it sums were:
/// a plane means nothing without its kind, so the two travel and are written together.
#[derive(Debug)]
pub struct WeightPlane {
    /// The plane: one every channel shares, or one per channel.
    pub map: QualityMap,
    /// What the frame weights the plane sums were.
    pub kind: WeightKind,
}

/// What the frame weights that a [`WeightPlane`] sums were, as the weighting stated them: what the
/// plane means, and the units it is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeightKind {
    /// Unit weights: the plane counts the frames that formed a pixel, each a drizzled frame's
    /// share of its drops, and carries no units.
    Count,
    /// Each frame's inverse noise variance: the plane is the inverse variance of each mean pixel,
    /// in the image's units to the power −2 (see
    /// [`StackProduct::weight`](crate::StackProduct::weight) for a median pixel).
    InverseVariance,
}

impl WeightKind {
    /// The units the weight plane is in, which set the scaling a FITS file stores it at.
    pub(crate) const fn units(self) -> WeightUnits {
        match self {
            Self::Count => WeightUnits::Unitless,
            Self::InverseVariance => WeightUnits::InverseSquare,
        }
    }
}
