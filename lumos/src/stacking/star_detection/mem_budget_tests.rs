//! Whether the star-detection pipeline respects its memory budget.
//!
//! Star detection has no explicit budget knob like the combine (`stacking::combine`) does; its
//! memory-safety guarantee is structural — a reused [`StarDetector`] recycles a fixed set of
//! image-sized scratch buffers through its [`BufferPool`], so detecting an unbounded number of
//! same-size frames costs a *constant* working set, not one that grows per frame. This is the
//! detector's analogue of the combine's "peak heap flat in the frame count": the pool footprint is
//! the ceiling, and every further frame must fit inside it.
//!
//! [`buffer_pool_working_set_stays_flat_in_frame_count`] asserts exactly that, deterministically,
//! with no live measurement. The at-scale peak-RSS counterpart is the `#[ignore]`d
//! `detect_memory_probe` in `mem_budget_probe`.
//!
//! [`StarDetector`]: super::detector::StarDetector
//! [`BufferPool`]: super::buffer_pool::BufferPool

use crate::stacking::star_detection::buffer_pool::PoolCounts;
use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::detector::StarDetector;
use crate::testing::synthetic::fixtures::star_field;

/// The detection working set at its high-water mark, for the default config: the buffers the pool
/// holds at rest once every stage has run and returned its scratch. Because buffers are recycled
/// across stages, this is the *peak concurrent* demand, not the sum of all acquisitions — at most 6
/// image-sized f32 planes live at once (grayscale + background/noise + detect-stage scratch), one
/// threshold bitmask, and the single label map. Pinned exactly so any change to the pipeline's
/// concurrent buffer demand surfaces here for review rather than silently growing peak heap.
const DEFAULT_WORKING_SET: PoolCounts = PoolCounts {
    floats: 6,
    bitmasks: 1,
    labels: 1,
};

/// A reused detector's buffer-pool working set must stay flat in the number of frames detected: the
/// pool never grows past its warmed high-water mark, and that mark is a small, image-bounded
/// constant. A per-frame buffer leak (a stage acquiring scratch it never releases) would push the
/// counts up without bound, making peak heap linear in the frame count — this catches it.
#[test]
fn buffer_pool_working_set_stays_flat_in_frame_count() {
    let (w, h) = (128, 128);
    // A handful of distinct fields (same dimensions, so the pool reuses rather than reallocates) so
    // every content-dependent detection path runs during warmup and the pool reaches its true
    // high-water mark before we start checking for growth.
    let frames: Vec<_> = (0..4)
        .map(|s| star_field(w, h, 60, 4200 + s).image)
        .collect();

    let mut detector = StarDetector::from_config(Config::default()).unwrap();

    // Warm up across every distinct field: after this the pool holds its full steady-state scratch.
    for frame in &frames {
        detector.detect(frame);
    }
    let baseline = detector
        .pool_counts()
        .expect("pool is populated after the first detect");
    assert_eq!(
        baseline, DEFAULT_WORKING_SET,
        "warmed pool footprint changed from the pinned working set — if this is an intentional \
         pipeline change, update DEFAULT_WORKING_SET; otherwise a stage's concurrent buffer demand \
         grew, raising peak heap"
    );

    // The guarantee: no matter how many more same-size frames we detect, the pool never grows past
    // the warmed working set. Acquire/release is balanced per stage, so a growing count means a leak.
    for i in 0..64 {
        detector.detect(&frames[i % frames.len()]);
        let c = detector.pool_counts().unwrap();
        assert!(
            c.floats <= baseline.floats
                && c.bitmasks <= baseline.bitmasks
                && c.labels <= baseline.labels,
            "pool grew on detection {i}: {c:?} exceeds the warmed baseline {baseline:?} — a scratch \
             buffer leaked per frame, so star detection's memory would scale with the frame count"
        );
    }
}
