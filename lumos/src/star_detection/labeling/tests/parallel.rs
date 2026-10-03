//! Masks tall enough to cut into several bands: every band count in `STRIP_COUNTS` puts its
//! boundaries somewhere else, so each case is stitched at many rows.

use crate::internals::prelude::*;
use crate::star_detection::labeling::tests::{Mask, STRIP_COUNTS, check};

const SIZE: Size2us = Size2us::new(400, 300);

#[test]
fn tall_masks_count_as_worked_out() {
    // (name, mask, 4-connected count, 8-connected count)
    let cases = [
        (
            "a vertical line through every band",
            Mask::from_fn(SIZE, |x, _| x == 200),
            1,
            1,
        ),
        (
            "a diagonal through every band",
            Mask::from_fn(SIZE, |x, y| x == y),
            300,
            1,
        ),
        (
            "a U joined only at its foot",
            Mask::from_fn(SIZE, |x, y| {
                ((x == 100 || x == 300) && (50..250).contains(&y))
                    || (y == 249 && (100..=300).contains(&x))
            }),
            1,
            1,
        ),
        (
            "a band of rows 60..70",
            Mask::from_fn(SIZE, |x, y| {
                (60..70).contains(&y) && (100..200).contains(&x)
            }),
            1,
            1,
        ),
        (
            "every other row",
            Mask::from_fn(SIZE, |_, y| y % 2 == 0),
            150,
            150,
        ),
        // Lines every 10 rows: one component each, through any band boundary they meet.
        (
            "horizontal lines",
            Mask::from_fn(SIZE, |x, y| y % 10 == 5 && (10..390).contains(&x)),
            30,
            30,
        ),
        (
            "a sparse grid of single pixels",
            Mask::from_fn(SIZE, |x, y| x % 10 == 5 && y % 10 == 5),
            40 * 30,
            40 * 30,
        ),
        // A quarter of the pixels, each its own component — its diagonal neighbours are odd in x
        // and y, so unset: the union-find holds one provisional label per run, here 30 000, sized
        // from the foreground count.
        (
            "a dense grid of single pixels",
            Mask::from_fn(SIZE, |x, y| x % 2 == 0 && y % 2 == 0),
            200 * 150,
            200 * 150,
        ),
        (
            "every pixel set",
            Mask::from_fn(Size2us::new(200, 200), |_, _| true),
            1,
            1,
        ),
        (
            "four corners of a large frame",
            Mask::from_fn(Size2us::new(1000, 1000), |x, y| {
                (x == 10 || x == 990) && (y == 10 || y == 990)
            }),
            4,
            4,
        ),
        (
            "one wide row",
            Mask::from_fn(Size2us::new(500, 1), |x, _| {
                [10..20, 100..110, 400..410]
                    .iter()
                    .any(|run| run.contains(&x))
            }),
            3,
            3,
        ),
        (
            "one tall column",
            Mask::from_fn(Size2us::new(1, 500), |_, y| {
                [10..20, 100..110, 400..410]
                    .iter()
                    .any(|run| run.contains(&y))
            }),
            3,
            3,
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

/// A 5×5 square across every band boundary the band counts produce, each in its own columns: every
/// square must stay one component however the rows are cut.
#[test]
fn squares_across_every_band_boundary_stay_whole() {
    let mut boundaries: Vec<usize> = STRIP_COUNTS
        .iter()
        .flat_map(|&strips| (1..strips).map(move |k| k * (SIZE.height / strips)))
        .collect();
    boundaries.sort_unstable();
    boundaries.dedup();
    let mask = Mask::from_fn(SIZE, |x, y| {
        boundaries.iter().enumerate().any(|(i, &boundary)| {
            (i * 10..i * 10 + 5).contains(&x) && (boundary - 2..boundary + 3).contains(&y)
        })
    });
    let labelled = check(&mask);
    assert_eq!(labelled.four.num_labels(), boundaries.len());
    assert!(
        labelled
            .four
            .components()
            .iter()
            .all(|component| component.area == 25)
    );
}
