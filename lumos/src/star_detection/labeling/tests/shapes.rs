//! Small shapes, each with its labels worked out by hand for both connectivities.

use crate::internals::prelude::*;
use crate::star_detection::labeling::tests::{Mask, check};

/// Expected labels, one string per row: `.` is background, a digit the label.
fn labels(rows: &[&str]) -> Vec<u32> {
    rows.iter()
        .flat_map(|row| {
            row.bytes()
                .map(|b| if b == b'.' { 0 } else { u32::from(b - b'0') })
        })
        .collect()
}

#[test]
fn shapes_label_as_worked_out() {
    // (name, mask, 4-connected labels, 8-connected labels). Components number in raster order of
    // their first pixel.
    type Shape = (
        &'static str,
        &'static [&'static str],
        &'static [&'static str],
        &'static [&'static str],
    );
    let shapes: &[Shape] = &[
        (
            "empty",
            &["....", "...."],
            &["....", "...."],
            &["....", "...."],
        ),
        (
            "single pixel",
            &["....", ".#..", "...."],
            &["....", ".1..", "...."],
            &["....", ".1..", "...."],
        ),
        (
            "filled",
            &["###", "###", "###"],
            &["111", "111", "111"],
            &["111", "111", "111"],
        ),
        (
            "horizontal line",
            &[".....", "###..", "....."],
            &[".....", "111..", "....."],
            &[".....", "111..", "....."],
        ),
        (
            "vertical line",
            &[".#.", ".#.", ".#.", ".#.", "..."],
            &[".1.", ".1.", ".1.", ".1.", "..."],
            &[".1.", ".1.", ".1.", ".1.", "..."],
        ),
        (
            "separate pixels number left to right",
            &["#.#.#."],
            &["1.2.3."],
            &["1.2.3."],
        ),
        (
            "two pixels across a row",
            &["#....#", "......"],
            &["1....2", "......"],
            &["1....2", "......"],
        ),
        (
            "L shape",
            &["#...", "#...", "##..", "...."],
            &["1...", "1...", "11..", "...."],
            &["1...", "1...", "11..", "...."],
        ),
        (
            "cross",
            &[".#.", "###", ".#."],
            &[".1.", "111", ".1."],
            &[".1.", "111", ".1."],
        ),
        // Two runs labelled apart on the first row, merged by the third: union-find.
        (
            "U shape",
            &["#...#", "#...#", "#####"],
            &["1...1", "1...1", "11111"],
            &["1...1", "1...1", "11111"],
        ),
        (
            "diagonal",
            &["#..", ".#.", "..#"],
            &["1..", ".2.", "..3"],
            &["1..", ".1.", "..1"],
        ),
        (
            "anti-diagonal",
            &["..#", ".#.", "#.."],
            &["..1", ".2.", "3.."],
            &["..1", ".1.", "1.."],
        ),
        ("corner touch", &["#.", ".#"], &["1.", ".2"], &["1.", ".1"]),
        (
            "squares touching at a corner",
            &["##..", "##..", "..##", "..##"],
            &["11..", "11..", "..22", "..22"],
            &["11..", "11..", "..11", "..11"],
        ),
        // Runs [0, 3) and [3, 6) on adjacent rows: no column shared, one diagonal.
        (
            "runs meeting at a diagonal",
            &["###....", "...###."],
            &["111....", "...222."],
            &["111....", "...111."],
        ),
        (
            "staircase",
            &["##....", "..##..", "....##"],
            &["11....", "..22..", "....33"],
            &["11....", "..11..", "....11"],
        ),
        // The left run reaches the right one only through the bottom row; the top-right run
        // overlaps nothing below it, even diagonally (x = 9 against 11).
        (
            "runs merging through a lower row",
            &["###........###", "..###..###....", "....####......"],
            &["111........222", "..111..111....", "....1111......"],
            &["111........222", "..111..111....", "....1111......"],
        ),
        (
            "checkerboard",
            &["#.#.", ".#.#", "#.#.", ".#.#"],
            &["1.2.", ".3.4", "5.6.", ".7.8"],
            &["1.1.", ".1.1", "1.1.", ".1.1"],
        ),
        (
            "touching every edge",
            &[
                "##........",
                ".........#",
                "..........",
                "#.........",
                "..........",
                "........##",
            ],
            &[
                "11........",
                ".........2",
                "..........",
                "3.........",
                "..........",
                "........44",
            ],
            &[
                "11........",
                ".........2",
                "..........",
                "3.........",
                "..........",
                "........44",
            ],
        ),
    ];

    for &(name, mask, four, eight) in shapes {
        let labelled = check(&Mask::ascii(mask));
        assert_eq!(labelled.four.labels(), labels(four), "{name}, 4-connected");
        assert_eq!(
            labelled.eight.labels(),
            labels(eight),
            "{name}, 8-connected"
        );
    }
}

/// Shapes too large to spell out label by label, by their component counts.
#[test]
fn larger_shapes_count_as_worked_out() {
    // A 9×9 spiral: the outer arm, the inner arm (one gap from it everywhere, so not even a
    // diagonal touches), and the lone centre. Concentric square rings: two. An 8×8 checkerboard:
    // 32 lone pixels 4-connected, one 8-connected.
    let spiral = Mask::ascii(&[
        "#########",
        "........#",
        "#.#####.#",
        "#.....#.#",
        "#.#.#.#.#",
        "#.#...#.#",
        "#.#####.#",
        "#.......#",
        "#########",
    ]);
    let rings = Mask::ascii(&[
        "###########",
        "#.........#",
        "#.........#",
        "#.........#",
        "#...###...#",
        "#...#.#...#",
        "#...###...#",
        "#.........#",
        "#.........#",
        "#.........#",
        "###########",
    ]);
    let checkerboard = Mask::from_fn(Size2us::new(8, 8), |x, y| (x + y) % 2 == 0);
    for (name, mask, four, eight) in [
        ("spiral", spiral, 3, 3),
        ("concentric rings", rings, 2, 2),
        ("8×8 checkerboard", checkerboard, 32, 1),
    ] {
        let labelled = check(&mask);
        assert_eq!(
            (labelled.four.num_labels(), labelled.eight.num_labels()),
            (four, eight),
            "{name}"
        );
    }
}

#[test]
fn zero_sized_masks_have_no_components() {
    for size in [Size2us::new(0, 10), Size2us::new(10, 0)] {
        let labelled = check(&Mask::from_fn(size, |_, _| true));
        assert_eq!(labelled.four.num_labels(), 0, "{size:?}");
        assert_eq!(labelled.eight.num_labels(), 0, "{size:?}");
    }
}
