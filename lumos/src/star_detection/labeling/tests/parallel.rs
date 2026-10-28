//! Masks tall enough to cut into several bands: every band count in `STRIP_COUNTS` puts its
//! boundaries somewhere else, so each case is stitched at many rows.

use crate::star_detection::labeling::tests::{Mask, STRIP_COUNTS, check};
use common::USize2;

const SIZE: USize2 = USize2::new(400, 300);

#[test]
fn tall_masks_count_as_worked_out() {
    // (name, mask, 4-connected count, 8-connected count)
    let cases = [
        (
            "a vertical line through every band",
            Mask::from_fn(SIZE, |at| at.x == 200),
            1,
            1,
        ),
        (
            "a diagonal through every band",
            Mask::from_fn(SIZE, |at| at.x == at.y),
            300,
            1,
        ),
        (
            "a U joined only at its foot",
            Mask::from_fn(SIZE, |at| {
                ((at.x == 100 || at.x == 300) && (50..250).contains(&at.y))
                    || (at.y == 249 && (100..=300).contains(&at.x))
            }),
            1,
            1,
        ),
        (
            "a band of rows 60..70",
            Mask::from_fn(SIZE, |at| {
                (60..70).contains(&at.y) && (100..200).contains(&at.x)
            }),
            1,
            1,
        ),
        (
            "every other row",
            Mask::from_fn(SIZE, |at| at.y % 2 == 0),
            150,
            150,
        ),
        // Lines every 10 rows: one component each, through any band boundary they meet.
        (
            "horizontal lines",
            Mask::from_fn(SIZE, |at| at.y % 10 == 5 && (10..390).contains(&at.x)),
            30,
            30,
        ),
        (
            "a sparse grid of single pixels",
            Mask::from_fn(SIZE, |at| at.x % 10 == 5 && at.y % 10 == 5),
            40 * 30,
            40 * 30,
        ),
        // A quarter of the pixels, each its own component — its diagonal neighbours are odd in x
        // and y, so unset: the union-find holds one provisional label per run, here 30 000, sized
        // from the foreground count.
        (
            "a dense grid of single pixels",
            Mask::from_fn(SIZE, |at| at.x % 2 == 0 && at.y % 2 == 0),
            200 * 150,
            200 * 150,
        ),
        (
            "every pixel set",
            Mask::from_fn(USize2::new(200, 200), |_| true),
            1,
            1,
        ),
        (
            "four corners of a large frame",
            Mask::from_fn(USize2::new(1000, 1000), |at| {
                (at.x == 10 || at.x == 990) && (at.y == 10 || at.y == 990)
            }),
            4,
            4,
        ),
        (
            "one wide row",
            Mask::from_fn(USize2::new(500, 1), |at| {
                [10..20, 100..110, 400..410]
                    .iter()
                    .any(|run| run.contains(&at.x))
            }),
            3,
            3,
        ),
        (
            "one tall column",
            Mask::from_fn(USize2::new(1, 500), |at| {
                [10..20, 100..110, 400..410]
                    .iter()
                    .any(|run| run.contains(&at.y))
            }),
            3,
            3,
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
    let mask = Mask::from_fn(SIZE, |at| {
        boundaries.iter().enumerate().any(|(i, &boundary)| {
            (i * 10..i * 10 + 5).contains(&at.x) && (boundary - 2..boundary + 3).contains(&at.y)
        })
    });
    let labelled = check(&mask);
    assert_eq!(labelled.four.label_count(), boundaries.len());
    assert!(
        labelled
            .four
            .components()
            .iter()
            .all(|component| component.area == 25)
    );
}
