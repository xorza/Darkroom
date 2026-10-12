//! Star detection, which plans nothing and must hold the same heap however many frames one
//! detector is given.

use lumos::{StarDetectionConfig, StarDetector};

use crate::session::Session;
use crate::star_field::StarField;

const SIDE: usize = 64;
const STARS: usize = 6;

/// A fresh detector under `config` over three frames holds exactly what it holds over four: the
/// first frames leave the scratch the detector keeps warm — the labeling's two row vectors swap
/// from row to row, so each reaches the longest row's runs only once it has held it — and a buffer
/// kept per frame beyond that would raise the fourth frame's peak. On one worker: the deblender
/// keeps a warm scratch for each of its jobs that ran at once, so on more its heap follows the
/// schedule rather than the frame count.
fn holds_the_same_heap_for_any_frame_count(config: &StarDetectionConfig) {
    let session = Session::on_workers("lumos_budget_detection", 1);
    let frame = StarField::new(SIDE, STARS, 0xDE7E).frame(0);
    let peak = |frames: usize| {
        session
            .measure(|| {
                let mut detector = StarDetector::from_config(config.clone()).unwrap();
                for _ in 0..frames {
                    drop(detector.detect(&frame));
                }
            })
            .peak_bytes
    };
    assert_eq!(peak(3), peak(4));
}

/// Each preset differs in the stages that hold planes: the iterative background refinement, the
/// FWHM estimate's extra detection, the multi-threshold deblender.
#[test]
fn the_default_detector_holds_the_same_heap_for_any_frame_count() {
    holds_the_same_heap_for_any_frame_count(&StarDetectionConfig::default());
}

#[test]
fn a_wide_field_detector_holds_the_same_heap_for_any_frame_count() {
    holds_the_same_heap_for_any_frame_count(&StarDetectionConfig::wide_field());
}

#[test]
fn a_high_resolution_detector_holds_the_same_heap_for_any_frame_count() {
    holds_the_same_heap_for_any_frame_count(&StarDetectionConfig::high_resolution());
}

#[test]
fn a_crowded_field_detector_holds_the_same_heap_for_any_frame_count() {
    holds_the_same_heap_for_any_frame_count(&StarDetectionConfig::crowded_field());
}

#[test]
fn a_precise_ground_detector_holds_the_same_heap_for_any_frame_count() {
    holds_the_same_heap_for_any_frame_count(&StarDetectionConfig::precise_ground());
}
