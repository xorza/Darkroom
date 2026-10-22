//! Reusable resources for star detection.
//!
//! The resources retain image buffers and stage-specific workspaces across detections.

use imaginarium::Buffer2;

use crate::bit_buffer2::BitBuffer2;
use crate::math::size2us::Size2us;
use crate::scratch_pool::{Pooled, ScratchPool};
use crate::star_detection::bg::workspace::BgWorkspace;
use crate::star_detection::convolution::filter_kernels::FilterKernels;
use crate::star_detection::deblend::deblend_buffers::DeblendBuffers;
use crate::star_detection::deblend::region::Region;
use crate::star_detection::duplicates::Duplicates;
use crate::star_detection::labeling::labeler::Labeler;
use crate::star_detection::star::Star;

/// Reusable planes and stage workspaces for star detection, kept across `detect()` calls so a
/// frame after the first allocates nothing but the stars it hands back.
///
/// Every plane is leased at the frame's dimensions and goes back to its pool when the lease drops,
/// so a stage keeps its planes in its own structs for as long as later stages read them. Contents
/// of a leased plane are **unspecified**: overwrite before reading.
#[derive(Debug, Default)]
pub(crate) struct DetectionResources {
    /// Grayscale, scratch, background, noise — anything one f32 plane wide.
    pub(crate) floats: ScratchPool<Buffer2<f32>>,
    /// Threshold masks, dilation scratch.
    bitmasks: ScratchPool<BitBuffer2>,
    pub(crate) labeler: Labeler,
    pub(crate) bg: BgWorkspace,
    /// The deblenders' working sets, one per rayon fold split.
    pub(crate) deblend: ScratchPool<DeblendBuffers>,
    /// The per-tile and per-star values a detection's medians sort, refilled for each.
    pub(crate) values: Vec<f32>,
    /// Their absolute deviations from the median, for a MAD that keeps the values.
    pub(crate) deviations: Vec<f32>,
    pub(crate) duplicates: Duplicates,
    pub(crate) kernels: FilterKernels,
    /// The candidate regions of a detection.
    pub(crate) regions: Vec<Region>,
    /// Each region's measured star, or none, before the stars that measured are gathered.
    pub(crate) measured: Vec<Option<Star>>,
    /// The stars the FWHM estimate measures.
    pub(crate) stars: Vec<Star>,
}

/// The f32 planes the pool holds at its high-water mark over every preset: the measurement plane,
/// its sky noise, the detection plane and the filter's pass scratch, which the detection plane's
/// noise takes over. `star_detection::tests::pool_working_set` pins it against the pool.
pub(super) const PEAK_FLOATS: usize = 4;

/// The bitmasks the pool holds at its high-water mark: saturation, threshold, the sources a
/// refined sky was measured around, and the pixels with no data of a frame that has any.
pub(super) const PEAK_BITMASKS: usize = 4;

/// The f32 planes the labeling's runs are charged: a run is 16 bytes in its strip and 12 in the
/// label map, so the runs hold under one f32 plane while fewer than one pixel in seven starts a
/// run — a mask of sources a few σ above the sky starts far fewer.
const LABEL_RUNS_PLANES: usize = 1;

impl DetectionResources {
    /// The bytes a detector's pool holds for frames of `size` at its high-water mark: its f32
    /// planes and the labeling's runs, a plane each, and its bitmasks, a bit a pixel each.
    pub(crate) const fn heap_bytes(size: Size2us) -> usize {
        (PEAK_FLOATS + LABEL_RUNS_PLANES) * size.pixel_count() * size_of::<f32>()
            + PEAK_BITMASKS * BitBuffer2::heap_bytes(size)
    }

    /// The dimensions every plane is leased at.
    pub(crate) fn dimensions(&self) -> Size2us {
        self.floats.shape()
    }

    pub(super) fn acquire_f32(&self) -> Pooled<Buffer2<f32>> {
        self.floats.acquire()
    }

    pub(super) fn acquire_bit(&self) -> Pooled<BitBuffer2> {
        self.bitmasks.acquire()
    }

    /// Serve frames of `dimensions`. A change drops the planes, and the workspaces whose size
    /// follows the frame's: the labeler, the background mesh and the deblenders. The per-tile and
    /// per-star vectors, the duplicate scratch and the filter kernels follow the star and tile
    /// counts, and stay.
    pub(crate) fn reshape(&mut self, dimensions: Size2us) {
        if self.dimensions() == dimensions {
            return;
        }
        self.floats.reshape(dimensions);
        self.bitmasks.reshape(dimensions);
        self.labeler = Labeler::default();
        self.bg.clear();
        self.deblend.clear();
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::math::size2us::Size2us;
    use crate::scratch_pool::ScratchPool;
    use crate::star_detection::resources::DetectionResources;

    #[cfg(test)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct BufferCounts {
        pub(crate) floats: usize,
        pub(crate) bitmasks: usize,
    }

    impl DetectionResources {
        /// Resources for frames of `dimensions`, for a stage a test or a bench runs on its own; a
        /// detector reshapes its own per frame.
        pub(crate) fn new(dimensions: Size2us) -> Self {
            Self {
                floats: ScratchPool::new(dimensions),
                bitmasks: ScratchPool::new(dimensions),
                ..Self::default()
            }
        }

        /// The planes the pools hold free.
        #[cfg(test)]
        pub(crate) fn buffer_counts(&self) -> BufferCounts {
            BufferCounts {
                floats: self.floats.pooled(),
                bitmasks: self.bitmasks.pooled(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::math::size2us::Size2us;
    use crate::star_detection::resources::DetectionResources;
    use crate::star_detection::resources::internals::BufferCounts;

    const fn counts(floats: usize, bitmasks: usize) -> BufferCounts {
        BufferCounts { floats, bitmasks }
    }

    /// Each kind of plane is leased at the resources' dimensions from a pool of its own: nothing
    /// is built up front, a dropped lease goes back to its kind's pool only, and the next lease of
    /// that kind is the same allocation.
    #[test]
    fn planes_lease_at_the_frame_dimensions_from_one_pool_per_kind() {
        let resources = DetectionResources::new(Size2us::new(64, 32));
        assert_eq!(resources.dimensions(), Size2us::new(64, 32));
        assert_eq!(resources.buffer_counts(), counts(0, 0));

        let plane = resources.acquire_f32();
        assert_eq!((plane.width(), plane.height()), (64, 32));
        let address = plane.as_ptr();
        drop(plane);
        assert_eq!(resources.buffer_counts(), counts(1, 0));
        assert_eq!(resources.acquire_f32().as_ptr(), address);

        let mask = resources.acquire_bit();
        assert_eq!(mask.size, Size2us::new(64, 32));
        let address = mask.words_ptr();
        drop(mask);
        assert_eq!(resources.buffer_counts(), counts(1, 1));
        assert_eq!(resources.acquire_bit().words_ptr(), address);
    }

    /// The same dimensions keep every plane; new ones drop them, and a plane leased before the
    /// change is not shelved when it drops after it.
    #[test]
    fn reshape_drops_the_planes_on_a_change_of_dimensions() {
        let mut resources = DetectionResources::new(Size2us::new(64, 64));
        drop((resources.acquire_f32(), resources.acquire_bit()));
        let outstanding = resources.acquire_f32();

        resources.reshape(Size2us::new(64, 64));
        assert_eq!(resources.buffer_counts(), counts(0, 1));

        resources.reshape(Size2us::new(128, 64));
        assert_eq!(resources.dimensions(), Size2us::new(128, 64));
        assert_eq!(resources.buffer_counts(), counts(0, 0));
        drop(outstanding);
        assert_eq!(resources.buffer_counts(), counts(0, 0));
        assert_eq!(resources.acquire_f32().width(), 128);
    }
}
