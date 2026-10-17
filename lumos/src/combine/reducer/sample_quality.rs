//! [`SampleQuality`]: what a reduced pixel reports beside its value.

use crate::stack_product::quality_planes::QualityPlanes;

/// What a reduced pixel reports beside its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SampleQuality {
    /// The value alone: the walk over the survivors would produce figures nothing reads, and it
    /// costs one pass over the frames per pixel.
    ValueOnly,
    /// The survivors' weight, the inverse variance of the value and the survivors' dispersion.
    Measured,
}

impl SampleQuality {
    /// What a combine writing `planes` measures of each pixel.
    pub(crate) const fn of(planes: QualityPlanes) -> Self {
        if planes.weight || planes.inverse_variance || planes.dispersion {
            Self::Measured
        } else {
            Self::ValueOnly
        }
    }
}
