use std::mem;
use std::panic::catch_unwind;

use common::Buffer2;

use crate::bit_buffer2::BitBuffer2;
use crate::internals::panic_message;
use common::USize2;
use glam::USizeVec2;

/// Every bit of `buffer` in row-major order.
fn iter_bits(buffer: &BitBuffer2) -> Vec<bool> {
    buffer.iter().collect()
}

#[test]
fn construction_aligns_rows_and_preserves_fill_values() {
    for (width, expected_stride, expected_words_per_row) in
        [(1, 128, 2), (64, 128, 2), (128, 128, 2), (129, 256, 4)]
    {
        let clear = BitBuffer2::new_default(USize2::new(width, 3));
        let filled = BitBuffer2::new_filled(USize2::new(width, 3), true);

        assert_eq!(clear.size, USize2::new(width, 3));
        assert_eq!(clear.stride, expected_stride);
        assert_eq!(clear.words_per_row(), expected_words_per_row);
        assert_eq!(clear.words.len(), expected_words_per_row * 3);
        assert_eq!(clear.size.pixel_count(), width * 3);
        assert_eq!(clear.count_ones(), 0);
        assert_eq!(filled.count_ones(), width * 3);
    }
}

#[test]
fn linear_coordinate_and_index_access_agree() {
    let mut buffer = BitBuffer2::new_default(USize2::new(65, 3));
    for idx in [0, 63, 64, 65, 129, 194] {
        buffer.set(idx, true);
    }
    buffer.set_at(USizeVec2::new(4, 2), true);

    let expected = [0, 63, 64, 65, 129, 134, 194];
    assert_eq!(buffer.count_ones(), expected.len());
    for idx in 0..buffer.size.pixel_count() {
        assert_eq!(buffer.get(idx), expected.contains(&idx), "index {idx}");
        assert_eq!(buffer[idx], buffer.get(idx));
    }
    assert!(buffer.get_at(USizeVec2::new(4, 2)));

    buffer.set_at(USizeVec2::new(4, 2), false);
    assert!(!buffer.get(134));
    assert_eq!(buffer.count_ones(), expected.len() - 1);
}

#[test]
fn slice_and_iteration_preserve_row_major_order() {
    let source = [
        true, false, true, false, false, true, false, true, true, false, false, true,
    ];
    let buffer = BitBuffer2::from_plane(&Buffer2::new((4, 3), source.to_vec()));
    assert_eq!(iter_bits(&buffer), source);

    // The set-bit walk visits exactly the true positions, in row-major order.
    let mut visited = Vec::new();
    buffer.for_each_set(|pos| visited.push(pos.y * 4 + pos.x));
    let expected: Vec<usize> = (0..source.len()).filter(|&i| source[i]).collect();
    assert_eq!(visited, expected);

    // A filled buffer has its padding set too, which the walk must not report; a row past one
    // word reaches its second.
    let filled = BitBuffer2::new_filled(USize2::new(70, 2), true);
    let mut count = 0;
    let mut last = USizeVec2::ZERO;
    filled.for_each_set(|pos| {
        count += 1;
        last = pos;
    });
    assert_eq!(count, 140);
    assert_eq!(last, USizeVec2::new(69, 1));
}

#[test]
fn every_zero_dimension_is_empty_and_iterates_to_nothing() {
    for (width, height) in [(0, 0), (0, 7), (7, 0), (0, usize::MAX), (usize::MAX, 0)] {
        let buffer = BitBuffer2::new_filled(USize2::new(width, height), true);

        assert!(buffer.size.width == 0 || buffer.size.height == 0);
        assert_eq!(buffer.size, USize2::new(width, height));
        assert_eq!(buffer.stride, 0);
        assert!(buffer.words.is_empty());
        assert_eq!(buffer.size.pixel_count(), 0);
        assert_eq!(buffer.count_ones(), 0);
        assert_eq!(iter_bits(&buffer), Vec::<bool>::new());
    }
}

/// Counts of pixels and of runs along the rows: a full 7 × 2 mask is 14 pixels in 2 runs, a row
/// per run, and a pixel cleared mid-row splits its run. A run across the 64-pixel word boundary of
/// a 130-wide row, pixels 63 and 64, is one; pixel 127 alone and 129, the row's last, are two
/// more; a run at the start of row 1 begins a run of its own after row 0 ended set.
#[test]
fn fill_count_clone_and_swap() {
    let mut first = BitBuffer2::new_default(USize2::new(7, 2));
    assert_eq!(first.count_ones(), 0);
    assert_eq!(first.count_runs(), 0);
    assert!(iter_bits(&first).iter().all(|&value| !value));

    first.fill(true);
    assert_eq!(first.count_ones(), 14);
    assert_eq!(first.count_runs(), 2);
    first.set_at(USizeVec2::new(3, 1), false);
    assert_eq!(first.count_ones(), 13);
    assert_eq!(first.count_runs(), 3);

    let mut wide = BitBuffer2::new_default(USize2::new(130, 2));
    for x in [63, 64, 127, 129] {
        wide.set_at(USizeVec2::new(x, 0), true);
    }
    wide.set_at(USizeVec2::new(0, 1), true);
    assert_eq!(wide.count_runs(), 4);

    let mut copy = first.clone();
    assert_eq!(iter_bits(&copy), iter_bits(&first));

    let mut clear = BitBuffer2::new_default(USize2::new(7, 2));
    mem::swap(&mut copy.words, &mut clear.words);
    assert_eq!(copy.count_ones(), 0);
    assert_eq!(clear.count_ones(), 13);
}

#[test]
fn construction_rejects_every_dimension_overflow_stage() {
    for (width, height, expected) in [
        (usize::MAX, 1, "BitBuffer2 row stride overflow"),
        (128, usize::MAX, "BitBuffer2 dimensions overflow"),
        (1, usize::MAX, "BitBuffer2 storage size overflow"),
    ] {
        let panic = catch_unwind(|| BitBuffer2::new_default(USize2::new(width, height)))
            .expect_err("overflowing dimensions must panic");
        assert!(
            panic_message(&*panic).contains(expected),
            "{width}x{height} did not report {expected}"
        );
    }
}

#[cfg(debug_assertions)]
#[test]
fn debug_access_rejects_out_of_bounds_indices_and_coordinates() {
    use std::panic::AssertUnwindSafe;

    let mut buffer = BitBuffer2::new_default(USize2::new(3, 2));

    assert!(catch_unwind(|| buffer.get(6)).is_err());
    assert!(catch_unwind(|| buffer.get_at(USizeVec2::new(3, 0))).is_err());
    assert!(catch_unwind(AssertUnwindSafe(|| buffer.set(6, true))).is_err());
    assert!(
        catch_unwind(AssertUnwindSafe(
            || buffer.set_at(USizeVec2::new(0, 2), true)
        ))
        .is_err()
    );
}

/// A width that is neither a whole word nor a whole 128-bit row: 100 real bits per row, stored in
/// two words, with 28 bits of padding after them. Readers must ignore those — `new_filled(_, true)`
/// sets them, so counting whole words would over-report.
const RAGGED: USize2 = USize2 {
    width: 100,
    height: 3,
};

/// A mask filled from `predicate`, the way a caller with nothing to recycle builds one.
fn from_predicate(size: USize2, predicate: impl Fn(usize) -> bool + Sync) -> BitBuffer2 {
    let mut buffer = BitBuffer2::new_default(size);
    buffer.fill_from_predicate(predicate);
    buffer
}

#[test]
fn fill_from_predicate_matches_bit_by_bit_setting() {
    // A pattern that crosses both the 64-bit word edge and the 100-bit row edge.
    let pattern = |i: usize| i.is_multiple_of(7) || i % 64 == 63;
    let built = from_predicate(RAGGED, pattern);

    let mut by_hand = BitBuffer2::new_default(RAGGED);
    for i in 0..by_hand.size.pixel_count() {
        by_hand.set(i, pattern(i));
    }
    assert_eq!(built.words, by_hand.words, "packing differs");
    assert_eq!(built.count_ones(), by_hand.count_ones());
}

#[test]
fn fill_from_predicate_handles_exact_word_and_empty_sizes() {
    let exact = from_predicate(USize2::new(64, 2), |i| i < 64);
    assert_eq!(exact.count_ones(), 64);
    assert!(exact.get(63) && !exact.get(64));

    let empty = from_predicate(USize2::new(0, 0), |_| true);
    assert_eq!(empty.count_ones(), 0);
    assert_eq!(empty.size.pixel_count(), 0);
}

/// Every way of setting pixels leaves the row padding clear, on every row shape: all three
/// strides are 128 bits, so 128 wide has no padding, 100 wide pads 28 bits inside the second word,
/// and 60 wide pads 4 bits of the first word and the whole second one. A full fill is the buffer
/// that setting every pixel builds, so whole-word counts and smears read 3 rows of 100 as 300
/// pixels: a dilation of a full buffer is the full buffer. The last word's mask is the width's
/// bits within it: 36 at 100, 60 at 60, all 64 at 128.
#[test]
fn the_padding_stays_clear_whatever_sets_the_pixels() {
    for (width, last_bits) in [(128, 64), (100, 36), (60, 60)] {
        let size = USize2::new(width, 3);
        assert!(BitBuffer2::new_default(size).padding_is_clear(), "{width}");
        let built = from_predicate(size, |_| true);
        assert_eq!(built.count_ones(), width * 3, "{width}");
        assert!(built.padding_is_clear(), "{width}");

        let mut poked = BitBuffer2::new_default(size);
        for i in 0..poked.size.pixel_count() {
            poked.set(i, true);
        }
        let mut filled = BitBuffer2::new_default(size);
        filled.fill(true);
        assert_eq!(poked, built, "set at {width}");
        assert_eq!(filled, built, "fill at {width}");
        assert_eq!(
            BitBuffer2::new_filled(size, true),
            built,
            "new_filled at {width}"
        );

        let mut dilated = filled.clone();
        dilated.dilate(5, &mut BitBuffer2::new_default(size));
        assert_eq!(dilated, built, "dilate at {width}");

        filled.fill_from_predicate(|i| i.is_multiple_of(3));
        assert_eq!(
            filled,
            from_predicate(size, |i| i.is_multiple_of(3)),
            "reuse at {width}"
        );

        let mask = built.last_word_mask();
        assert_eq!(mask.count_ones(), last_bits, "{width}");
        assert_eq!(built.row_words(2)[(width - 1) / 64], mask, "{width}");
    }
}

/// A square finds a set bit inside it and only there, across word boundaries and clipped at the
/// buffer's edges. On 130 × 20 with (64, 5) set — the first bit of a row's second word — the square
/// of 3 about (61, 5) reaches column 64 and finds it, about (60, 5) stops at 63; about (64, 8) it
/// reaches row 5, about (64, 9) row 6. With (129, 19) set, a square about (126, 16) reaches the
/// corner. About (0, 0) the square of 5 is clipped to the first 6 rows and columns: (5, 5) is in
/// it, (6, 0) is not.
#[test]
fn a_square_finds_the_bits_inside_it() {
    let mut buffer = BitBuffer2::new_default(USize2::new(130, 20));
    buffer.set_at(USizeVec2::new(64, 5), true);
    assert!(buffer.any_in_square(USizeVec2::new(61, 5), 3));
    assert!(!buffer.any_in_square(USizeVec2::new(60, 5), 3));
    assert!(buffer.any_in_square(USizeVec2::new(64, 8), 3));
    assert!(!buffer.any_in_square(USizeVec2::new(64, 9), 3));
    buffer.set_at(USizeVec2::new(129, 19), true);
    assert!(buffer.any_in_square(USizeVec2::new(126, 16), 3));
    assert!(!buffer.any_in_square(USizeVec2::new(125, 16), 3));

    let mut corner = BitBuffer2::new_default(USize2::new(130, 20));
    corner.set_at(USizeVec2::new(5, 5), true);
    assert!(corner.any_in_square(USizeVec2::new(0, 0), 5));
    let mut outside = BitBuffer2::new_default(USize2::new(130, 20));
    outside.set_at(USizeVec2::new(6, 0), true);
    assert!(!outside.any_in_square(USizeVec2::new(0, 0), 5));
}

/// Cleared to a smaller size, a buffer keeps its words and holds no bit of the larger one, padding
/// included; cleared to a larger one, it takes that size's layout. 200 px rows pad to 256 bits,
/// four words a row: 3 rows are 12 words, 130 px rows of 2 are 2 · 4 = 8, and 400 px rows of 2
/// pad to 512 bits, 8 words a row, so 16.
#[test]
fn clear_to_reuses_the_words_and_clears_every_bit() {
    let mut buffer = BitBuffer2::new_filled(USize2::new(200, 3), true);
    assert_eq!(buffer.words.len(), 12);
    let held = buffer.words.as_ptr();

    buffer.clear_to(USize2::new(130, 2));
    assert_eq!(buffer.size, USize2::new(130, 2));
    assert_eq!(buffer.words.len(), 8);
    assert_eq!(
        buffer.words.as_ptr(),
        held,
        "a smaller size reuses the allocation"
    );
    assert!(buffer.words.iter().all(|&word| word == 0));
    assert!(buffer.padding_is_clear());
    buffer.set_at(USizeVec2::new(129, 1), true);
    assert_eq!(buffer.count_ones(), 1);

    buffer.clear_to(USize2::new(400, 2));
    assert_eq!((buffer.size, buffer.words.len()), (USize2::new(400, 2), 16));
    assert_eq!(buffer.count_ones(), 0);
}
