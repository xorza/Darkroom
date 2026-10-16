//! The detector's pool of image-sized scratch, pinned to what the memory planner charges for it.
//!
//! A reused [`StarDetector`] recycles a fixed set of image-sized scratch buffers through its
//! [`DetectionResources`]. The planner charges [`DetectionResources::heap_bytes`] for it, from the
//! pool's peak counts, which only means anything while they match what the pool holds:
//! [`WORKING_SET`] pins the pool, and the counts are checked against the pin. That the detector's
//! heap stays the same from frame to frame is the `memory_budget` integration test's.
//!
//! [`StarDetector`]: crate::star_detection::star_detector::StarDetector
//! [`DetectionResources`]: crate::star_detection::resources::DetectionResources

use rayon::prelude::*;

use crate::internals::synthetic::fixtures::star_field;
use crate::math::size2us::Size2us;
use crate::star_detection::resources::internals::BufferCounts;
use crate::star_detection::resources::{DetectionResources, PEAK_BITMASKS, PEAK_FLOATS};
use crate::star_detection::star_detection_config::StarDetectionConfig;
use crate::star_detection::star_detector::StarDetector;

/// The detection working set at its high-water mark over every preset: the buffers the pool holds
/// at rest once every stage has run and returned its scratch. Because buffers are recycled across
/// stages, this is the *peak concurrent* demand, not the sum of all acquisitions. Pinned exactly so
/// any change to the pipeline's concurrent buffer demand surfaces here for review rather than
/// silently growing peak heap.
const WORKING_SET: BufferCounts = BufferCounts {
    floats: 4,
    bitmasks: 3,
};

/// The bitmask a frame with pixels of no data adds to [`WORKING_SET`], which these synthetic fields
/// do not carry.
const NO_DATA_MASK: usize = 1;

/// The planner charges [`DetectionResources::heap_bytes`] from the pool's peak counts, which only
/// means anything while they match what the pool actually holds. Tying the two together here is
/// what stops the planner's figure from being a number nobody can check: a stage that grows its
/// scratch fails [`warmed_pool_holds_the_pinned_working_set`] first, and raising `WORKING_SET` to
/// match then fails this until the peak counts are raised too. A 64 × 64 frame's pool holds five
/// planes of 16 KiB — four f32 planes and the labeling's runs — and four bitmasks of 64 rows of
/// 128 bits, 1 KiB each.
#[test]
fn pinned_working_set_matches_what_the_memory_planner_charges() {
    let BufferCounts { floats, bitmasks } = WORKING_SET;
    assert_eq!(floats, PEAK_FLOATS);
    assert_eq!(bitmasks + NO_DATA_MASK, PEAK_BITMASKS);
    assert_eq!(
        DetectionResources::heap_bytes(Size2us::new(64, 64)),
        5 * 16_384 + 4 * 1024
    );
}

/// A detector's pool, warmed on two fields so every content-dependent stage has run, holds
/// [`WORKING_SET`] at most over every preset, and that under some preset: the presets differ in the
/// stages that hold planes — the iterative background refinement, the FWHM estimate's extra
/// detection, the multi-threshold deblender.
#[test]
fn warmed_pool_holds_the_pinned_working_set() {
    let size = Size2us::new(128, 128);
    let frames: Vec<_> = (0..2)
        .map(|s| star_field(size, 30, 4200 + s).image)
        .collect();
    let presets = [
        StarDetectionConfig::default(),
        StarDetectionConfig::wide_field(),
        StarDetectionConfig::high_resolution(),
        StarDetectionConfig::crowded_field(),
        StarDetectionConfig::precise_ground(),
    ];
    let warmed: Vec<BufferCounts> = presets
        .into_par_iter()
        .map(|config| {
            let mut detector = StarDetector::from_config(config).unwrap();
            for frame in &frames {
                detector.detect(frame);
            }
            detector.buffer_counts()
        })
        .collect();
    let peak = warmed.iter().fold(
        BufferCounts {
            floats: 0,
            bitmasks: 0,
        },
        |peak, counts| BufferCounts {
            floats: peak.floats.max(counts.floats),
            bitmasks: peak.bitmasks.max(counts.bitmasks),
        },
    );
    assert_eq!(
        peak, WORKING_SET,
        "warmed pool footprint changed from the pinned working set — if this is an intentional \
         pipeline change, update WORKING_SET; otherwise a stage's concurrent buffer demand grew, \
         raising peak heap"
    );
}
