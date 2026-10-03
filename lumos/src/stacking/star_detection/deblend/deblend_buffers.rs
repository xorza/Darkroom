//! [`DeblendBuffers`]: the scratch one deblending job reuses from component to component.

use crate::stacking::star_detection::deblend::Pixel;
use crate::stacking::star_detection::deblend::multi_threshold::TreeBuffers;

/// The scratch one deblending job reuses across the components it handles, leased from the
/// detector's pool so a frame after the first allocates nothing here.
#[derive(Debug, Default)]
pub(crate) struct DeblendBuffers {
    /// Local-maxima candidates of one component, before they are ranked.
    pub(crate) maxima: Vec<Pixel>,
    /// The multi-threshold deblender's working sets.
    pub(crate) tree: TreeBuffers,
}
