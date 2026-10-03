//! [`DeblendBuffers`]: the scratch one deblending job reuses from component to component.

use crate::star_detection::deblend::Pixel;
use crate::star_detection::deblend::component::Assignment;
use crate::star_detection::deblend::multi_threshold::TreeBuffers;

/// The scratch one deblending job reuses across the components it handles, leased from the
/// detector's pool so a frame after the first allocates nothing here.
#[derive(Debug, Default)]
pub(crate) struct DeblendBuffers {
    /// Local-maxima candidates of one component, before they are ranked.
    pub(crate) maxima: Vec<Pixel>,
    /// The peaks a deblender keeps, which the component is then split at.
    pub(crate) peaks: Vec<Pixel>,
    /// Which pixels of the component's box hold a kept local maximum, all `false` between
    /// components.
    pub(crate) occupied: Vec<bool>,
    /// The per-peak boxes and areas of that split.
    pub(crate) assignment: Assignment,
    /// The multi-threshold deblender's working sets.
    pub(crate) tree: TreeBuffers,
}
