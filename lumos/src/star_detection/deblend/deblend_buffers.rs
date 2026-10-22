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

/// The bytes a set holds for each pixel of the component it splits at most: the pixel, 12 B; its
/// links to the rows above and below, 8 B; whether it holds a kept maximum, 1 B; and the tree's
/// object, visit, active, member and queue indices, 20 B.
const PIXEL_BYTES: usize = 41;

impl DeblendBuffers {
    /// The component pixels a set keeps room for from one component to the next: a star's
    /// component is far smaller. A set that grew past it for a larger one, a galaxy or a nebula,
    /// gives the room back after it ([`Self::release`]), so the largest component a frame holds
    /// does not stay in the pool for the run.
    pub(crate) const KEPT_PIXELS: usize = 4096;

    /// The bytes a set keeps between components of a frame of `pixels` at most: room for
    /// [`Self::KEPT_PIXELS`], or for the whole frame when it holds fewer.
    pub(crate) const fn kept_bytes(pixels: usize) -> usize {
        let kept = if pixels < Self::KEPT_PIXELS {
            pixels
        } else {
            Self::KEPT_PIXELS
        };
        kept * PIXEL_BYTES
    }

    /// Give back the room past [`Self::KEPT_PIXELS`] a large component took. The regions gathered
    /// stay.
    pub(crate) fn release(&mut self) {
        self.pixels.shrink_to(Self::KEPT_PIXELS);
        self.tree.shrink_to(Self::KEPT_PIXELS);
        self.occupied.clear();
        self.occupied.shrink_to(Self::KEPT_PIXELS);
    }
}

#[cfg(test)]
mod tests {
    use crate::math::urect::URect;
    use crate::math::vec2us::Vec2us;
    use crate::star_detection::deblend::deblend_buffers::DeblendBuffers;
    use crate::star_detection::deblend::region::Region;

    /// A set that grew to a component of a million pixels gives the room back to
    /// [`DeblendBuffers::KEPT_PIXELS`] of them, in every vector of each pixel, and keeps the
    /// regions it gathered.
    #[test]
    fn a_released_set_keeps_room_for_a_star_and_its_regions() {
        let mut buffers = DeblendBuffers::default();
        buffers.pixels.reserve_per_pixel(1 << 20);
        buffers.tree.reserve_per_pixel(1 << 20);
        buffers.occupied.reserve(1 << 20);
        let region = Region {
            bbox: URect::new(Vec2us::new(1, 2), Vec2us::new(3, 4)),
            peak: Vec2us::new(2, 3),
            area: 4,
        };
        buffers.regions.push(region);
        buffers.release();
        let kept = DeblendBuffers::KEPT_PIXELS;
        assert!(buffers.pixels.per_pixel_capacity() <= kept);
        assert!(buffers.tree.per_pixel_capacity() <= kept);
        assert!(buffers.occupied.capacity() <= kept);
        assert_eq!(buffers.regions.len(), 1);
    }
}
