//! [`DeblendBuffers`]: the scratch one deblending job reuses from component to component.

use crate::scratch_pool::Shapeless;
use crate::star_detection::deblend::assignment::Assignment;
use crate::star_detection::deblend::component_pixels::ComponentPixels;
use crate::star_detection::deblend::multi_threshold::TreeBuffers;
use crate::star_detection::deblend::pixel::Pixel;
use crate::star_detection::deblend::region::Region;

/// The scratch one deblending job reuses across the components it handles, leased from the
/// detector's pool so a frame after the first allocates nothing here.
#[derive(Debug, Default)]
pub(crate) struct DeblendBuffers {
    /// The pixels of the component being deblended.
    pub(crate) pixels: ComponentPixels,
    /// Local-maxima candidates of one component, before they are ranked.
    pub(crate) maxima: Vec<Pixel>,
    /// The peaks a deblender keeps, which the component is then split at.
    pub(crate) peaks: Vec<Pixel>,
    /// Which pixels of the component hold a kept local maximum.
    pub(crate) occupied: Vec<bool>,
    /// The per-peak boxes and areas of that split.
    pub(crate) assignment: Assignment,
    /// The multi-threshold deblender's working sets.
    pub(crate) tree: TreeBuffers,
    /// The regions this job's components split into, gathered across them.
    pub(crate) regions: Vec<Region>,
}

impl Shapeless for DeblendBuffers {}
