//! Masks built for the run extraction: runs across 64-bit word boundaries, full and alternating
//! words, and runs that overlap, miss or skip rows.

use crate::star_detection::labeling::run::Run;
use crate::star_detection::labeling::tests::{Mask, check};
use crate::star_detection::star_detection_config::extraction_config::Connectivity;
use common::USize2;

#[test]
fn search_window_widens_by_one_each_way_for_eight_connectivity() {
    let run = Run {
        start: 5,
        end: 10,
        label: 0,
    };
    assert_eq!(run.search_window(Connectivity::Four), 5..10);
    assert_eq!(run.search_window(Connectivity::Eight), 4..11);

    // At the left edge the widened start saturates rather than wrapping to u32::MAX.
    let at_edge = Run {
        start: 0,
        end: 3,
        label: 0,
    };
    assert_eq!(at_edge.search_window(Connectivity::Eight), 0..4);
}

#[test]
fn run_patterns_count_as_worked_out() {
    // (name, mask, 4-connected count, 8-connected count)
    let cases = [
        (
            "a run across the first word boundary (x = 62..67)",
            Mask::from_fn(USize2::new(70, 3), |at| {
                at.y == 1 && (62..67).contains(&at.x)
            }),
            1,
            1,
        ),
        (
            "a run across the second word boundary (x = 126..131)",
            Mask::from_fn(USize2::new(140, 3), |at| {
                at.y == 1 && (126..131).contains(&at.x)
            }),
            1,
            1,
        ),
        (
            "a run spanning three words",
            Mask::from_fn(USize2::new(200, 5), |at| {
                at.y == 2 && (10..190).contains(&at.x)
            }),
            1,
            1,
        ),
        (
            "two full words",
            Mask::from_fn(USize2::new(128, 3), |at| at.y == 1),
            1,
            1,
        ),
        (
            "alternating bits in a word",
            Mask::from_fn(USize2::new(64, 3), |at| at.y == 1 && at.x % 2 == 0),
            32,
            32,
        ),
        (
            "three runs in one row",
            Mask::from_fn(USize2::new(100, 3), |at| {
                at.y == 1
                    && [5..15, 30..40, 60..70]
                        .iter()
                        .any(|run| run.contains(&at.x))
            }),
            3,
            3,
        ),
        (
            "runs overlapping on adjacent rows",
            Mask::from_fn(USize2::new(50, 4), |at| {
                (at.y == 1 && (10..30).contains(&at.x)) || (at.y == 2 && (20..40).contains(&at.x))
            }),
            1,
            1,
        ),
        (
            "runs apart on adjacent rows",
            Mask::from_fn(USize2::new(50, 4), |at| {
                (at.y == 1 && (5..15).contains(&at.x)) || (at.y == 2 && (25..35).contains(&at.x))
            }),
            2,
            2,
        ),
        (
            "runs touching only at a diagonal (x = 14 against 15)",
            Mask::from_fn(USize2::new(50, 4), |at| {
                (at.y == 1 && (5..15).contains(&at.x)) || (at.y == 2 && (15..25).contains(&at.x))
            }),
            2,
            1,
        ),
        (
            "runs with empty rows between",
            Mask::from_fn(USize2::new(50, 10), |at| {
                (at.y == 2 || at.y == 7) && (10..20).contains(&at.x)
            }),
            2,
            2,
        ),
    ];

    for (name, mask, four, eight) in cases {
        let labelled = check(&mask);
        assert_eq!(
            (labelled.four.label_count(), labelled.eight.label_count()),
            (four, eight),
            "{name}"
        );
    }
}
