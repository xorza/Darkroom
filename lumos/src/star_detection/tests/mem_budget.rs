//! Whether the star-detection pipeline respects its memory budget.
//!
//! Star detection has no explicit budget knob like the combine (`combine`) does; its
//! memory-safety guarantee is structural — a reused [`StarDetector`] recycles a fixed set of
//! image-sized scratch buffers through its [`DetectionResources`], so detecting an unbounded number
//! of same-size frames costs a *constant* working set, not one that grows per frame. This is the
//! detector's analogue of the combine's "peak heap flat in the frame count": the pool footprint is
//! the ceiling, and every further frame must fit inside it.
//!
//! [`buffer_working_set_stays_flat_in_frame_count`] asserts exactly that, deterministically,
//! with no live measurement. The at-scale peak-RSS counterpart is the `#[ignore]`d
//! `detect_memory_probe` in `mem_budget_probe`.
//!
//! [`StarDetector`]: crate::star_detection::detector::StarDetector
//! [`DetectionResources`]: crate::star_detection::resources::DetectionResources

use rayon::prelude::*;

use crate::internals::synthetic::fixtures::star_field;
use crate::math::size2us::Size2us;
use crate::memory::DETECTION_WORKING_PLANES;
use crate::star_detection::config::Config;
use crate::star_detection::detector::StarDetector;
use crate::star_detection::detector::internals::buffer_counts_for;
use crate::star_detection::resources::internals::BufferCounts;

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

/// The plane the labeling's runs are charged: a run is 16 bytes in its strip and 12 in the label
/// map, so the runs hold under one f32 plane while fewer than one pixel in seven starts a run — a
/// mask of sources a few σ above the sky starts far fewer.
const LABEL_RUNS: usize = 1;

/// The memory planner charges [`DETECTION_WORKING_PLANES`] image-sized planes for a frame in
/// detection, which only means anything while that matches what the pool actually holds. Tying the
/// two together here is what stops the planner's figure from being a number nobody can check: a
/// stage that grows its scratch fails [`buffer_working_set_stays_flat_in_frame_count`] first, and
/// raising `WORKING_SET` to match then fails this until the planner is raised too.
#[test]
fn pinned_working_set_matches_what_the_memory_planner_charges() {
    let BufferCounts { floats, bitmasks } = WORKING_SET;

    // A bitmask is one bit per pixel against an f32 plane's 32, and the planner rounds each up to
    // a whole plane rather than model a fraction — so a plain sum is the figure it should carry.
    assert_eq!(
        floats + LABEL_RUNS + bitmasks + NO_DATA_MASK,
        DETECTION_WORKING_PLANES
    );
}

/// A reused detector's buffer-pool working set must stay flat in the number of frames detected: the
/// pool never grows past its warmed high-water mark, and that mark is a small, image-bounded
/// constant. A per-frame buffer leak (a stage acquiring scratch it never releases) would push the
/// counts up without bound, making peak heap linear in the frame count — this catches it.
///
/// Every preset runs, because they differ in the stages that hold planes: the iterative background
/// refinement, the FWHM estimate's extra detection, the multi-threshold deblender.
#[test]
fn buffer_working_set_stays_flat_in_frame_count() {
    let size = Size2us::new(128, 128);
    // A handful of distinct fields (same dimensions, so the pool reuses rather than reallocates) so
    // every content-dependent detection path runs during warmup and the pool reaches its true
    // high-water mark before we start checking for growth.
    let frames: Vec<_> = (0..4)
        .map(|s| star_field(size, 60, 4200 + s).image)
        .collect();

    // Each preset runs its own detector, so the presets run in parallel.
    let presets = [
        ("default", Config::default()),
        ("wide_field", Config::wide_field()),
        ("high_resolution", Config::high_resolution()),
        ("crowded_field", Config::crowded_field()),
        ("precise_ground", Config::precise_ground()),
    ];
    let baselines: Vec<BufferCounts> = presets
        .into_par_iter()
        .map(|(name, config)| {
            let mut detector = StarDetector::from_config(config).unwrap();

            // Warm up across every distinct field: after this the pool holds its full steady-state
            // scratch.
            for frame in &frames {
                detector.detect(frame);
            }
            let baseline = buffer_counts_for(&detector)
                .expect("resources are populated after the first detect");

            // No matter how many more same-size frames we detect, the pool never grows past the
            // warmed working set. Acquire/release is balanced per stage, so a leak grows the count
            // on the next detection: one more pass over every field shows it.
            for (i, frame) in frames.iter().enumerate() {
                detector.detect(frame);
                let c = buffer_counts_for(&detector).unwrap();
                assert!(
                    c.floats <= baseline.floats && c.bitmasks <= baseline.bitmasks,
                    "{name}: pool grew on detection {i}: {c:?} exceeds the warmed baseline \
                     {baseline:?} — a scratch buffer leaked per frame, so star detection's memory \
                     would scale with the frame count"
                );
            }
            baseline
        })
        .collect();
    let peak = baselines.iter().fold(
        BufferCounts {
            floats: 0,
            bitmasks: 0,
        },
        |peak, baseline| BufferCounts {
            floats: peak.floats.max(baseline.floats),
            bitmasks: peak.bitmasks.max(baseline.bitmasks),
        },
    );
    assert_eq!(
        peak, WORKING_SET,
        "warmed pool footprint changed from the pinned working set — if this is an intentional \
         pipeline change, update WORKING_SET; otherwise a stage's concurrent buffer demand grew, \
         raising peak heap"
    );
}
