//! [`LightWorker`]: what one slot of a registered run carries from one light to the next.

use crate::calibration::cosmic_ray::cosmic_ray_detector::CosmicRayDetector;
use crate::registration::resample::WarpResult;
use crate::star_detection::star_detection_config::StarDetectionConfig;
use crate::star_detection::star_detector::StarDetector;

/// What one slot of a registered run carries from one light to the next: its star detector's
/// warmed buffers, its cosmic-ray detector's, and the set its lights warp into — in either
/// schedule, so a slot's buffers are faulted in once.
#[derive(Debug)]
pub(super) struct LightWorker {
    pub(super) detector: StarDetector,
    pub(super) cosmic_ray: CosmicRayDetector,
    pub(super) warped: Option<WarpResult>,
}

impl LightWorker {
    /// One worker for each of `slots` lights in flight, detecting under `detection`, which the run
    /// validated at its start.
    pub(super) fn slots(detection: &StarDetectionConfig, slots: usize) -> Vec<Self> {
        debug_assert!(slots > 0, "a run detects one light at a time at least");
        (0..slots)
            .map(|_| Self {
                detector: StarDetector::new(detection.clone()),
                cosmic_ray: CosmicRayDetector::new(),
                warped: None,
            })
            .collect()
    }
}
