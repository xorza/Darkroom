use crate::testing::prelude::*;
use std::any::Any;
use std::panic::catch_unwind;

use crate::bit_buffer2::BitBuffer2;

fn panic_text(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[test]
fn construction_aligns_rows_and_preserves_fill_values() {
    for (width, expected_stride, expected_words_per_row) in
        [(1, 128, 2), (64, 128, 2), (128, 128, 2), (129, 256, 4)]
    {
        let clear = BitBuffer2::new_default(Size2us::new(width, 3));
        let filled = BitBuffer2::new_filled(Size2us::new(width, 3), true);

        assert_eq!(clear.size, Size2us::new(width, 3));
        assert_eq!(clear.stride, expected_stride);
        assert_eq!(clear.words_per_row(), expected_words_per_row);
        assert_eq!(clear.words.len(), expected_words_per_row * 3);
        assert_eq!(clear.len, width * 3);
        assert_eq!(clear.count_ones(), 0);
        assert_eq!(filled.count_ones(), width * 3);
    }
}

#[test]
fn linear_coordinate_and_index_access_agree() {
    let mut buffer = BitBuffer2::new_default(Size2us::new(65, 3));
    for index in [0, 63, 64, 65, 129, 194] {
        buffer.set(index, true);
    }
    buffer.set_at(Vec2us::new(4, 2), true);

    let expected = [0, 63, 64, 65, 129, 134, 194];
    assert_eq!(buffer.count_ones(), expected.len());
    for index in 0..buffer.len {
        assert_eq!(
            buffer.get(index),
            expected.contains(&index),
            "index {index}"
        );
        assert_eq!(buffer[index], buffer.get(index));
    }
    assert!(buffer[(4, 2)]);

    buffer.set_at(Vec2us::new(4, 2), false);
    assert!(!buffer.get(134));
    assert_eq!(buffer.count_ones(), expected.len() - 1);
}

#[test]
fn slice_iteration_and_conversion_preserve_row_major_order() {
    let source = [
        true, false, true, false, false, true, false, true, true, false, false, true,
    ];
    let buffer = BitBuffer2::from_slice(Size2us::new(4, 3), &source);
    let mut iter = buffer.iter();

    assert_eq!(iter.len(), 12);
    assert_eq!(iter.next(), Some(true));
    assert_eq!(iter.len(), 11);
    assert_eq!(iter.collect::<Vec<_>>(), source[1..]);
    assert_eq!(Vec::<bool>::from(&buffer), source);
    assert_eq!(Vec::<bool>::from(buffer), source);
}

#[test]
fn every_zero_dimension_is_empty_and_iterates_to_nothing() {
    for (width, height) in [(0, 0), (0, 7), (7, 0), (0, usize::MAX), (usize::MAX, 0)] {
        let buffer = BitBuffer2::new_filled(Size2us::new(width, height), true);

        assert!(buffer.size.width == 0 || buffer.size.height == 0);
        assert_eq!(buffer.size, Size2us::new(width, height));
        assert_eq!(buffer.stride, 0);
        assert!(buffer.words.is_empty());
        assert_eq!(buffer.len, 0);
        assert_eq!(buffer.count_ones(), 0);
        assert_eq!(buffer.iter().len(), 0);
        assert_eq!(buffer.iter().next(), None);
        assert_eq!(Vec::<bool>::from(&buffer), Vec::<bool>::new());
    }
}

#[test]
fn fill_count_copy_and_swap_ignore_padding() {
    let mut first = BitBuffer2::new_default(Size2us::new(7, 2));
    first.words[0] |= 1 << 7;
    assert_eq!(first.count_ones(), 0);
    assert!(first.iter().all(|value| !value));

    first.fill(true);
    assert_eq!(first.count_ones(), 14);
    first.set_at(Vec2us::new(3, 1), false);
    assert_eq!(first.count_ones(), 13);

    let mut copy = BitBuffer2::new_default(Size2us::new(7, 2));
    copy.copy_from(&first);
    assert_eq!(Vec::<bool>::from(&copy), Vec::<bool>::from(&first));

    let mut clear = BitBuffer2::new_default(Size2us::new(7, 2));
    std::mem::swap(&mut copy.words, &mut clear.words);
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
        let panic = catch_unwind(|| BitBuffer2::new_default(Size2us::new(width, height)))
            .expect_err("overflowing dimensions must panic");
        assert!(
            panic_text(panic).contains(expected),
            "{width}x{height} did not report {expected}"
        );
    }
}

#[test]
fn from_slice_rejects_a_mismatched_length() {
    let panic = catch_unwind(|| BitBuffer2::from_slice(Size2us::new(2, 2), &[true, false, true]))
        .expect_err("mismatched data length must panic");
    assert!(panic_text(panic).contains("data length 3 does not match dimensions 2x2=4"));
}

#[cfg(debug_assertions)]
#[test]
fn debug_access_rejects_out_of_bounds_indices_and_coordinates() {
    use std::panic::AssertUnwindSafe;

    let mut buffer = BitBuffer2::new_default(Size2us::new(3, 2));

    assert!(catch_unwind(|| buffer.get(6)).is_err());
    assert!(catch_unwind(|| buffer.get_at(Vec2us::new(3, 0))).is_err());
    assert!(catch_unwind(AssertUnwindSafe(|| buffer.set(6, true))).is_err());
    assert!(catch_unwind(AssertUnwindSafe(|| buffer.set_at(Vec2us::new(0, 2), true))).is_err());
}

/// A width that is neither a whole word nor a whole 128-bit row: 100 real bits per row, stored in
/// two words, with 28 bits of padding after them. Readers must ignore those — `new_filled(_, true)`
/// sets them, so counting whole words would over-report.
const RAGGED: Size2us = Size2us {
    width: 100,
    height: 3,
};

#[test]
fn from_predicate_matches_bit_by_bit_setting() {
    // A pattern that crosses both the 64-bit word edge and the 100-bit row edge.
    let pattern = |i: usize| i % 7 == 0 || i % 64 == 63;
    let built = BitBuffer2::from_predicate(RAGGED, pattern);

    let mut by_hand = BitBuffer2::new_default(RAGGED);
    for i in 0..by_hand.len {
        by_hand.set(i, pattern(i));
    }
    assert_eq!(built.words, by_hand.words, "packing differs");
    assert_eq!(built.count_ones(), by_hand.count_ones());
}

#[test]
fn from_predicate_handles_exact_word_and_empty_sizes() {
    let exact = BitBuffer2::from_predicate(Size2us::new(64, 2), |i| i < 64);
    assert_eq!(exact.count_ones(), 64);
    assert!(exact.get(63) && !exact.get(64));

    let empty = BitBuffer2::from_predicate(Size2us::new(0, 0), |_| true);
    assert_eq!(empty.count_ones(), 0);
    assert_eq!(empty.len, 0);
}

#[test]
fn padding_bits_never_reach_the_count() {
    // `new_filled(_, true)` sets whole words, so 3 rows of 100 bits are stored as 384 set bits.
    let mut mask = BitBuffer2::new_filled(RAGGED, true);
    assert_eq!(mask.count_ones(), 300, "300 real bits, not the 384 stored");
    assert!(
        mask.words.iter().map(|w| w.count_ones()).sum::<u32>() > 300,
        "the padding really is set"
    );

    for i in 0..mask.len {
        mask.set(i, false);
    }
    assert_eq!(mask.count_ones(), 0);
    assert!(
        mask.words.iter().any(|&w| w != 0),
        "padding survives untouched"
    );
}
