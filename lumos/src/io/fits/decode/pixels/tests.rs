use common::CancelToken;
use fits_well::image::{Scaling, View as FitsView};

use crate::error::Cancelled;
use crate::io::fits::decode::pixels::{ChunkScan, Conversion, SampleScan};
use crate::io::null_policy::PlaneNulls;

const IDENTITY: Scaling = Scaling {
    bscale: 1.0,
    bzero: 0.0,
    blank: None,
};

/// What [`convert`] gives back: the converted samples, and their scan.
#[derive(Debug)]
struct Converted {
    samples: Vec<f32>,
    scan: ChunkScan,
}

/// `samples` through [`Conversion::scan`], with the converted samples beside the scan.
fn convert(samples: FitsView<'_>, scaling: Scaling, divisor: f32) -> Converted {
    let mut out = vec![0.0; samples.len()];
    let scan = Conversion {
        scaling,
        divisor: f64::from(divisor),
        cancel: &CancelToken::never(),
    }
    .scan(samples, &mut out)
    .unwrap();
    Converted { samples: out, scan }
}

#[test]
fn cancellation_stops_chunk_validation() {
    let cancel = CancelToken::new();
    cancel.cancel();
    assert_eq!(
        Conversion {
            scaling: IDENTITY,
            divisor: 1.0,
            cancel: &cancel,
        }
        .scan(FitsView::F32(&[1.0, 2.0]), &mut [0.0; 2])
        .unwrap_err(),
        Cancelled
    );
}

#[test]
fn a_unit_divisor_accepts_every_finite_value_and_changes_none() {
    // The float-FITS path. Nothing is out of range to this pass — a negative calibration
    // residual and an undivided ADU value are both legitimate.
    let pixels = [-5.0, 0.0, 0.5, 2.0, 255.0, 65_535.0];
    let Converted { samples: out, scan } = convert(FitsView::F32(&pixels), IDENTITY, 1.0);
    assert_eq!(scan.nulls, None);
    assert_eq!(scan.scan.maximum, 65_535.0);
    assert_eq!(
        scan.scan.raw_bits, 0,
        "a float sample has no stored integer"
    );
    assert_eq!(out, pixels);
}

#[test]
fn normalizing_maps_the_declared_span_onto_the_unit_interval() {
    // BITPIX = 16 with BZERO = 2¹⁵, BSCALE = 1: divisor |1| × (2¹⁶ − 1) = 65535, so the
    // unsigned span 0..=65535 lands exactly on [0, 1].
    let unsigned_convention = Scaling {
        bzero: 32_768.0,
        ..IDENTITY
    };
    let Converted {
        samples: unsigned,
        scan,
    } = convert(
        FitsView::I16(&[-32_768, -16_384, 0, 32_767]),
        unsigned_convention,
        65_535.0,
    );
    // The stored integers −32768, −16384, 0 and 32767: the last is odd, so they share no
    // trailing zero and the step is one stored unit.
    assert_eq!(scan.scan.raw_bits.trailing_zeros(), 0);
    assert_eq!(scan.scan.integer_step(1.0), 1.0);
    assert_eq!(scan.scan.maximum, 1.0);
    // Each is the quotient rounded once: the endpoints exactly, 16384/65535 and 32768/65535
    // to the nearest f32.
    assert_eq!(
        unsigned,
        [
            0.0,
            (16_384.0f64 / 65_535.0) as f32,
            (32_768.0f64 / 65_535.0) as f32,
            1.0
        ]
    );

    // The same divisor puts a signed frame on [-0.5, 0.5] around its own zero: the scale is
    // applied without an offset, so a negative sample stays negative.
    let Converted {
        samples: signed, ..
    } = convert(FitsView::I16(&[-32_768, 0, 32_767]), IDENTITY, 65_535.0);
    assert_eq!(
        signed,
        [
            (-32_768.0f64 / 65_535.0) as f32,
            0.0,
            (32_767.0f64 / 65_535.0) as f32
        ]
    );
}

/// A stored integer past `2²⁴` reaches the quotient whole: 2²⁴ + 1 over 65535 is
/// 256.00392156…, whose nearest f32 is 256.00394. Narrowed to f32 first it would be 2²⁴, and
/// the quotient 256.0039. Its bits are read whole too, so integers that are all multiples of
/// 16 past 2²⁴ step by 16.
#[test]
fn a_stored_integer_past_the_f32_mantissa_rounds_once() {
    let raw = (1i32 << 24) + 1;
    let Converted { samples: out, .. } = convert(FitsView::I32(&[raw]), IDENTITY, 65_535.0);
    assert_eq!(out[0], (f64::from(raw) / 65_535.0) as f32);
    assert_ne!(out[0], (raw as f32) / 65_535.0);

    let Converted { scan, .. } = convert(
        FitsView::I32(&[(1 << 24) + 16, (1 << 24) + 48]),
        IDENTITY,
        1.0,
    );
    assert_eq!(scan.scan.integer_step(1.0), 16.0);
}

/// `BLANK` is a null and no stored integer: the samples 4 and 8 share two trailing zeros, which
/// the sentinel −1, all ones, would otherwise erase.
#[test]
fn the_blank_sentinel_is_a_null_and_stays_out_of_the_integer_scan() {
    let scaling = Scaling {
        blank: Some(-1),
        ..IDENTITY
    };
    let Converted { samples: out, scan } = convert(FitsView::I16(&[4, -1, 8]), scaling, 1.0);
    assert!(out[1].is_nan());
    assert_eq!([out[0], out[2]], [4.0, 8.0]);
    assert_eq!(
        scan.nulls,
        Some(PlaneNulls {
            count: 1,
            first_idx: 1,
        })
    );
    assert_eq!(scan.scan.integer_step(1.0), 4.0);
}

/// With no `BLANK`, every stored value is a sample. Under the unsigned-64 convention
/// (`BZERO = 2^63`), stored `i64::MIN` is physical 0 and stored `i64::MAX` is `2^64 − 1`.
#[test]
fn without_blank_the_most_negative_stored_integer_is_a_sample() {
    let unsigned = Scaling {
        bzero: 2f64.powi(63),
        ..IDENTITY
    };
    let Converted { samples: out, scan } =
        convert(FitsView::I64(&[i64::MIN, i64::MAX]), unsigned, 1.0);
    assert_eq!(out, [0.0, 2f32.powi(64)]);
    assert_eq!(scan.nulls, None);
}

#[test]
fn a_span_that_overflows_a_finite_sample_is_reported_as_a_null() {
    // The divide runs before the finite test, so a divisor small enough to push a finite
    // sample past f32::MAX is caught by the same pass instead of reaching the image.
    let Converted { scan, .. } = convert(FitsView::F32(&[1.0e30, 0.0]), IDENTITY, 1.0e-30);
    assert_eq!(
        scan.nulls,
        Some(PlaneNulls {
            count: 1,
            first_idx: 0,
        })
    );
}

#[test]
fn nan_and_both_infinities_are_counted_after_the_divide() {
    // Nulls survive the divide, which is what lets the test run after it rather than before:
    // a NaN or ±inf divided by any span is still one, and is still counted here.
    let pixels = [0.0, f32::NAN, 5.0, f32::INFINITY, f32::NEG_INFINITY];
    let Converted { scan, .. } = convert(FitsView::F32(&pixels), IDENTITY, 65_535.0);
    assert_eq!(
        scan.nulls,
        Some(PlaneNulls {
            count: 3,
            first_idx: 1,
        })
    );
    // The maximum is over the finite samples only: 5/65535, not infinity.
    assert_eq!(scan.scan.maximum, (5.0f64 / 65_535.0) as f32);
}

/// The shared trailing zeros of the stored integers give the step: stored values that are all
/// multiples of 16 step by 16 × |BSCALE|; with every stored integer zero, the container's.
#[test]
fn the_integer_step_is_the_shared_trailing_zeros() {
    let shifted = SampleScan {
        raw_bits: 0b1_0000 | 0b11_0000 | 0b1111_1111_0000,
        maximum: 1.0,
    };
    assert_eq!(shifted.integer_step(0.5), 8.0);
    assert_eq!(SampleScan::EMPTY.integer_step(2.0), 2.0);
}

#[test]
fn summaries_from_two_chunks_combine_into_the_planes_index_space() {
    // Row-chunk 0 holds one null at 2; row-chunk 1 starts 8 samples in and holds two, at its
    // own 0 and 3 — pixels 8 and 11 of the plane. Merged: three nulls, first at 2.
    let first = PlaneNulls {
        count: 1,
        first_idx: 2,
    };
    let second = PlaneNulls {
        count: 2,
        first_idx: 0,
    };
    assert_eq!(
        first.merge(second.offset_by(8)),
        PlaneNulls {
            count: 3,
            first_idx: 2,
        }
    );
    // The offset moves only the location, and the merge takes the earlier one whichever side
    // it arrives on.
    assert_eq!(second.offset_by(8).first_idx, 8);
    assert_eq!(second.offset_by(8).merge(first).first_idx, 2);
}
