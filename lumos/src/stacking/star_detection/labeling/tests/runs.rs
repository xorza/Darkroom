//! Masks built for the run extraction: runs across 64-bit word boundaries, full and alternating
//! words, and runs that overlap, miss or skip rows.

use crate::stacking::star_detection::config::detection_config::Connectivity;
use crate::stacking::star_detection::labeling::run::Run;
use crate::stacking::star_detection::labeling::tests::{Mask, check};
use crate::testing::prelude::*;

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
            Mask::from_fn(Size2us::new(70, 3), |x, y| y == 1 && (62..67).contains(&x)),
            1,
            1,
        ),
        (
            "a run across the second word boundary (x = 126..131)",
            Mask::from_fn(Size2us::new(140, 3), |x, y| {
                y == 1 && (126..131).contains(&x)
            }),
            1,
            1,
        ),
        (
            "a run spanning three words",
            Mask::from_fn(Size2us::new(200, 5), |x, y| {
                y == 2 && (10..190).contains(&x)
            }),
            1,
            1,
        ),
        (
            "two full words",
            Mask::from_fn(Size2us::new(128, 3), |_, y| y == 1),
            1,
            1,
        ),
        (
            "alternating bits in a word",
            Mask::from_fn(Size2us::new(64, 3), |x, y| y == 1 && x % 2 == 0),
            32,
            32,
        ),
        (
            "three runs in one row",
            Mask::from_fn(Size2us::new(100, 3), |x, y| {
                y == 1 && [5..15, 30..40, 60..70].iter().any(|run| run.contains(&x))
            }),
            3,
            3,
        ),
        (
            "runs overlapping on adjacent rows",
            Mask::from_fn(Size2us::new(50, 4), |x, y| {
                (y == 1 && (10..30).contains(&x)) || (y == 2 && (20..40).contains(&x))
            }),
            1,
            1,
        ),
        (
            "runs apart on adjacent rows",
            Mask::from_fn(Size2us::new(50, 4), |x, y| {
                (y == 1 && (5..15).contains(&x)) || (y == 2 && (25..35).contains(&x))
            }),
            2,
            2,
        ),
        (
            "runs touching only at a diagonal (x = 14 against 15)",
            Mask::from_fn(Size2us::new(50, 4), |x, y| {
                (y == 1 && (5..15).contains(&x)) || (y == 2 && (15..25).contains(&x))
            }),
            2,
            1,
        ),
        (
            "runs with empty rows between",
            Mask::from_fn(Size2us::new(50, 10), |x, y| {
                (y == 2 || y == 7) && (10..20).contains(&x)
            }),
            2,
            2,
        ),
    ];

    for (name, mask, four, eight) in cases {
        let labelled = check(&mask);
        assert_eq!(
            (labelled.four.num_labels(), labelled.eight.num_labels()),
            (four, eight),
            "{name}"
        );
    }
}
