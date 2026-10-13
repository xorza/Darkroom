use common::CancelToken;

use crate::error::Cancelled;
use crate::io::fits::decode::pixels::{
    ChunkScan, FILL_MEDIAN_SAMPLES, NullSummary, SampleScan, convert_and_scan, fill_nulls,
};
use fits_well::image::{Scaling, View as FitsView};

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

/// `samples` through [`convert_and_scan`], with the converted samples beside the scan.
fn convert(samples: FitsView<'_>, scaling: Scaling, divisor: f32) -> Converted {
    let mut out = vec![0.0; samples.len()];
    let scan =
        convert_and_scan(samples, scaling, divisor, &mut out, &CancelToken::never()).unwrap();
    Converted { samples: out, scan }
}

#[test]
fn cancellation_stops_chunk_validation() {
    let cancel = CancelToken::new();
    cancel.cancel();
    assert_eq!(
        convert_and_scan(
            FitsView::F32(&[1.0, 2.0]),
            IDENTITY,
            1.0,
            &mut [0.0; 2],
            &cancel
        )
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
        Some(NullSummary {
            count: 1,
            first_index: 1,
        })
    );
    assert_eq!(scan.scan.integer_step(1.0), 4.0);
}

#[test]
fn a_span_that_overflows_a_finite_sample_is_reported_as_a_null() {
    // The divide runs before the finite test, so a divisor small enough to push a finite
    // sample past f32::MAX is caught by the same pass instead of reaching the image.
    let Converted { scan, .. } = convert(FitsView::F32(&[1.0e30, 0.0]), IDENTITY, 1.0e-30);
    assert_eq!(
        scan.nulls,
        Some(NullSummary {
            count: 1,
            first_index: 0,
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
        Some(NullSummary {
            count: 3,
            first_index: 1,
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
    let first = NullSummary {
        count: 1,
        first_index: 2,
    };
    let second = NullSummary {
        count: 2,
        first_index: 0,
    };
    assert_eq!(
        first.merge(second.offset_by(8)),
        NullSummary {
            count: 3,
            first_index: 2,
        }
    );
    // The offset moves only the location, and the merge takes the earlier one whichever side
    // it arrives on.
    assert_eq!(second.offset_by(8).first_index, 8);
    assert_eq!(second.offset_by(8).merge(first).first_index, 2);
}

#[test]
fn nulls_are_filled_with_the_median_of_the_finite_samples() {
    // Finite samples 1, 2, 3, 4, 10 — five of them, so the median is the middle one, 3. Both
    // nulls take it, and no finite sample moves.
    let mut samples = [1.0, f32::NAN, 2.0, 3.0, f32::INFINITY, 4.0, 10.0];
    fill_nulls(&mut samples, 2);
    assert_eq!(samples, [1.0, 3.0, 2.0, 3.0, 3.0, 4.0, 10.0]);

    // Zero fills a plane with no finite sample to borrow a level from: nothing else is more
    // right, and the mask is what records that none of it is data.
    let mut empty = [f32::NAN; 3];
    fill_nulls(&mut empty, 3);
    assert_eq!(empty, [0.0; 3]);
}

#[test]
fn the_fills_median_is_sampled_rather_than_taken_over_a_whole_plane() {
    // Twice the sample bound of valid data, so the stride is exactly 2 and the scratch holds
    // `FILL_MEDIAN_SAMPLES` values instead of a second copy of the plane — the allocation the
    // decode's memory preflight does not budget for.
    //
    // Valid samples are the ramp 0..200_000. Every second one is 0, 2, … 199_998, whose median
    // is the mean of its two middle values (99_998 and 100_000) = 99_999. The whole ramp's
    // median is 99_999.5, so the bound costs half a unit in two hundred thousand.
    const VALID: usize = 2 * FILL_MEDIAN_SAMPLES;
    let mut samples: Vec<f32> = (0..VALID).map(|index| index as f32).collect();
    samples.push(f32::NAN);
    fill_nulls(&mut samples, 1);
    assert_eq!(samples[VALID], 99_999.0);

    // The stride runs over the finite samples, not over plane positions: masking the whole
    // first half would otherwise spend that half of the quota on pixels that are dropped
    // anyway, and draw the median from the tail alone.
    let mut lopsided: Vec<f32> = (0..VALID)
        .map(|index| {
            if index < VALID / 2 {
                f32::NAN
            } else {
                index as f32
            }
        })
        .collect();
    fill_nulls(&mut lopsided, VALID / 2);
    // The surviving ramp is 100_000..200_000, whose median is 149_999.5; sampling every one of
    // them (stride 1, since the survivors are exactly the bound) reproduces it exactly.
    assert_eq!(lopsided[0], 149_999.5);
}
