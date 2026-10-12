//! Bounded detector reuse for frame-parallel pipeline stages.

use crate::error::InvalidConfigField;
use crate::star_detection::star_detection_config::StarDetectionConfig;
use crate::star_detection::star_detector::StarDetector;

/// One [`StarDetector`] per concurrency slot, reused across batches so each batch inherits the
/// previous one's warmed buffer pools.
///
/// The detectors are the slots of a bounded map rather than a
/// [`ScratchPool`](crate::scratch_pool::ScratchPool): there is one per frame in flight, each built
/// from the caller's config, and building every one up front is what makes an invalid
/// configuration fail here rather than from inside the parallel closure.
#[derive(Debug)]
pub(crate) struct DetectorPool {
    detectors: Vec<StarDetector>,
}

impl DetectorPool {
    /// Build `max_concurrent` detectors from one configuration, rejecting an invalid one before
    /// any frame is touched.
    pub(crate) fn from_config(
        config: &StarDetectionConfig,
        max_concurrent: usize,
    ) -> Result<Self, InvalidConfigField> {
        assert!(max_concurrent > 0, "max_concurrent must be > 0");
        let detectors = (0..max_concurrent)
            .map(|_| StarDetector::from_config(config.clone()))
            .collect::<Result<_, _>>()?;
        Ok(Self { detectors })
    }

    /// The detectors, one per concurrent slot of a [`FrameIntake`] map: each carries its warmed
    /// buffers from one frame to the next it happens to pick up. Which frames a given detector
    /// sees is not fixed: a detector takes whatever is next when it frees up.
    ///
    /// [`FrameIntake`]: crate::ingest::frame_intake::FrameIntake
    pub(crate) fn slots(&mut self) -> &mut [StarDetector] {
        &mut self.detectors
    }

    /// One detector of the pool, for a frame detected alone.
    pub(crate) fn first(&mut self) -> &mut StarDetector {
        &mut self.detectors[0]
    }
}
