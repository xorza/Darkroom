//! Bounded detector reuse for frame-parallel pipeline stages.

use crate::error::InvalidConfigField;
use crate::star_detection::config::Config;
use crate::star_detection::detector::StarDetector;

/// One [`StarDetector`] per concurrency slot, reused across batches so each batch inherits the
/// previous one's warmed buffer pools.
///
/// Deliberately not a [`JobScratchPool`](crate::concurrency::job_scratch_pool::JobScratchPool), and the type system
/// will not stop you: `StarDetector` implements `Default`, so `JobScratchPool<StarDetector>`
/// compiles — and then quietly hands out detectors built from `Config::default()` instead of the
/// caller's detection config, because the pool fills gaps with `T::default()`. Building every
/// slot from `config` up front is also what makes an invalid configuration fail here rather than
/// from inside the parallel closure.
#[derive(Debug)]
pub(crate) struct DetectorPool {
    detectors: Vec<StarDetector>,
}

impl DetectorPool {
    /// Build `max_concurrent` detectors from one configuration, rejecting an invalid one before
    /// any frame is touched.
    pub(crate) fn from_config(
        config: &Config,
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

    /// Each detector beside one of `slots`, which the job keeps from one frame to the next it
    /// takes: a worker's warp buffers, for one.
    pub(crate) fn paired_with<'s, S>(
        &'s mut self,
        slots: &'s mut [S],
    ) -> Vec<(&'s mut StarDetector, &'s mut S)> {
        assert_eq!(slots.len(), self.detectors.len(), "one slot per detector");
        self.detectors.iter_mut().zip(slots.iter_mut()).collect()
    }

    /// One detector of the pool, for a frame detected alone.
    pub(crate) fn first(&mut self) -> &mut StarDetector {
        &mut self.detectors[0]
    }
}
