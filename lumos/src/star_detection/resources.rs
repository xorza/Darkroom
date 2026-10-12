//! Reusable resources for star detection.
//!
//! The resources retain image buffers and stage-specific workspaces across detections.

use crate::bit_buffer2::BitBuffer2;
use crate::math::size2us::Size2us;
use crate::scratch_pool::{Pooled, ScratchPool};
use crate::star_detection::background::workspace::BackgroundWorkspace;
use crate::star_detection::convolution::FilterKernels;
use crate::star_detection::deblend::deblend_buffers::DeblendBuffers;
use crate::star_detection::labeling::labeler::Labeler;
use crate::star_detection::star_detector::stages::filter::DuplicateScratch;
use imaginarium::Buffer2;

/// Reusable planes and stage workspaces for star detection, kept across `detect()` calls so a
/// frame after the first allocates nothing.
///
/// Every plane is leased at the frame's dimensions and goes back to its pool when the lease drops,
/// so a stage keeps its planes in its own structs for as long as later stages read them. Contents
/// of a leased plane are **unspecified**: overwrite before reading.
#[derive(Debug, Default)]
pub(crate) struct DetectionResources {
    /// Grayscale, scratch, background, noise — anything one f32 plane wide.
    floats: ScratchPool<Buffer2<f32>>,
    /// Threshold masks, dilation scratch.
    bitmasks: ScratchPool<BitBuffer2>,
    pub(crate) labeler: Labeler,
    pub(crate) background: BackgroundWorkspace,
    /// The deblenders' working sets, one per rayon fold split.
    pub(crate) deblend: ScratchPool<DeblendBuffers>,
    /// The per-tile and per-star values a detection's medians sort, refilled for each.
    pub(crate) values: Vec<f32>,
    /// Their absolute deviations from the median, for a MAD that keeps the values.
    pub(crate) deviations: Vec<f32>,
    pub(crate) duplicates: DuplicateScratch,
    pub(crate) kernels: FilterKernels,
}

impl DetectionResources {
    /// The dimensions every plane is leased at.
    pub(crate) fn dimensions(&self) -> Size2us {
        self.floats.shape()
    }

    pub(crate) fn acquire_f32(&self) -> Pooled<Buffer2<f32>> {
        self.floats.acquire()
    }

    pub(crate) fn acquire_bit(&self) -> Pooled<BitBuffer2> {
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
        self.background.clear();
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
        pub floats: usize,
        pub bitmasks: usize,
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
        let address = mask.words.as_ptr();
        drop(mask);
        assert_eq!(resources.buffer_counts(), counts(1, 1));
        assert_eq!(resources.acquire_bit().words.as_ptr(), address);
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
