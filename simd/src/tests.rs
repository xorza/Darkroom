use std::any;
use std::f64::consts::PI;

use crate::backend::portable::Portable;
use crate::f32x8::{F32_LANES, F32x8};
use crate::f64x4::{F64_LANES, F64x4};
use crate::halves::Halves;
use crate::i32x8::I32x8;
use crate::isa::Isa;
use crate::isa_tier::IsaTier;
use crate::kernel::Kernel;
use crate::mask8::Mask8;
use crate::math::Math;
use crate::u8x32::U8x32;
use crate::u16x16::U16x16;

/// Rows that put every special value against every other across the battery's pairs: signed
/// zeros, NaN, infinities, subnormals, and ordinary values of both signs and many magnitudes.
const ROWS: [[f32; F32_LANES]; 4] = [
    [
        1.5,
        -2.25,
        0.0,
        -0.0,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        1e-40,
    ],
    [-0.0, 0.0, f32::NAN, 3.0, 1.0, f32::INFINITY, 7.5, -1e-40],
    [1e30, -1e-30, 0.1, 0.2, 0.3, 1.0e7, -3.0, 6.0],
    [1.0, 1.0, 1.0, -1.0, f32::NAN, 0.5, 2.0, 1e-38],
];

/// Bytes at both ends and the middle of the range, in every lane position.
const BYTES: [u8; 32] = [
    0, 1, 2, 127, 128, 129, 254, 255, 3, 64, 200, 17, 250, 251, 8, 99, 255, 0, 128, 7, 1, 254, 33,
    44, 55, 66, 77, 88, 99, 111, 222, 233,
];

/// Words at both ends, at the byte boundary, at the sign boundary, and at the products that carry
/// into the high half.
const WORDS: [[u16; 16]; 2] = [
    [
        0, 1, 255, 256, 257, 32767, 32768, 65535, 65280, 65281, 4096, 1000, 12345, 54321, 2, 40000,
    ],
    [
        65535, 65535, 2, 255, 65281, 2, 2, 65535, 257, 65281, 16, 64, 7, 3, 32768, 40000,
    ],
];

/// f64 lanes that round across ties, below zero, at and past `high`, and NaN.
const ROUNDED: [[f64; 4]; 2] = [[0.5, 1.5, 2.5, -0.5], [f64::NAN, 254.5, 65535.4, 1e30]];

/// `i32` lanes past the `u16` range both ways, at `i32`'s ends, and beside the luma weights.
const INTS: [[i32; 8]; 2] = [
    [0, -1, 65535, 65536, i32::MAX, i32::MIN, 16_777_217, -70],
    [13_933, 46_871, 4_732, 255, 256, -65536, 70_000, 3],
];

/// Indices before, inside and past a sixteen-entry table, at `i32`'s ends; as byte offsets into
/// [`GATHER_BYTES`], 15 reads its last four bytes and 16 its last three.
const GATHER_IDX: [i32; 8] = [0, 15, 16, -1, 7, i32::MIN, i32::MAX, 3];

const GATHER_BYTES: [u8; 19] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10,
    0xF1, 0xF2, 0xF3,
];

const TABLE: [f32; 16] = [
    10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0, 19.0, 20.0, 21.0, 22.0, 23.0, 24.0, 25.0,
];

/// What one op produced, every lane widened to f64 (exactly, NaN to NaN).
#[derive(Debug)]
struct Outcome {
    op: String,
    lanes: Vec<f64>,
}

/// Every op of the vector traits, and every [`Math`] function, over every pair of [`ROWS`].
#[derive(Debug)]
struct Battery;

impl Kernel for Battery {
    type Output = Vec<Outcome>;

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) -> Vec<Outcome> {
        let mut out = Vec::new();
        let mut f32s = |op: String, v: S::F32| {
            out.push(Outcome {
                op,
                lanes: v.to_array().map(f64::from).to_vec(),
            });
        };
        let mut f64s = Vec::new();
        let mut u16s = Vec::new();
        let mut i32s = Vec::new();
        let mut scalars = Vec::new();

        for (i, a) in ROWS.iter().enumerate() {
            let va = isa.load_f32(a);
            f32s(format!("sqrt {i}"), va.sqrt());
            f32s(format!("floor {i}"), va.floor());
            f32s(format!("abs {i}"), va.abs());
            let split = va.frexp();
            f32s(format!("frexp mantissa {i}"), split.mantissa);
            f32s(format!("frexp exponent {i}"), split.exponent);
            f32s(format!("ln {i}"), isa.ln_f32(va));
            let bounds = isa.load_f32(&[1.0, 0.5, 2.0, f32::INFINITY, 1e-40, 3.0, 1e30, 0.25]);
            f32s(format!("min positive {i}"), va.min_positive(bounds));
            f32s(format!("asinh {i}"), isa.asinh_f32(va));
            f32s(format!("lookup {i}"), isa.lookup_f32(&TABLE, va));
            f32s(
                format!("lookup scaled {i}"),
                isa.lookup_f32(&TABLE, va * isa.splat_f32(5.3)),
            );
            f32s(format!("partial load {i}"), isa.load_f32_partial(&a[..5]));
            let mut stored = [7.0f32; 3];
            va.store_partial(&mut stored);
            f32s(format!("partial store {i}"), isa.load_f32_partial(&stored));
            scalars.push((format!("reduce {i}"), f64::from(va.reduce_sum())));

            let halves = va.widen();
            for (half, v) in [("low", halves.low), ("high", halves.high)] {
                f64s.push((format!("widen {half} {i}"), v));
                f64s.push((format!("sqrt {half} {i}"), v.sqrt()));
                f64s.push((format!("floor {half} {i}"), v.floor()));
                f64s.push((format!("exp {half} {i}"), isa.exp_f64(v)));
                f64s.push((format!("ln {half} {i}"), isa.ln_f64(v)));
                let split = v.frexp();
                f64s.push((format!("frexp mantissa {half} {i}"), split.mantissa));
                f64s.push((format!("frexp exponent {half} {i}"), split.exponent));
                scalars.push((format!("reduce {half} {i}"), v.reduce_sum()));
            }

            for (j, b) in ROWS.iter().enumerate() {
                let vb = isa.load_f32(b);
                let vc = isa.load_f32(&ROWS[(i + j + 1) % ROWS.len()]);
                f32s(format!("{i} + {j}"), va + vb);
                f32s(format!("{i} - {j}"), va - vb);
                f32s(format!("{i} * {j}"), va * vb);
                f32s(format!("{i} / {j}"), va / vb);
                f32s(format!("{i} mul_add {j}"), va.mul_add(vb, vc));
                f32s(format!("{i} max {j}"), va.max(vb));
                f32s(format!("{i} min {j}"), va.min(vb));
                f32s(format!("{i} gt {j}"), va.lanes_gt(vb).select(va, vb));
                f32s(format!("{i} lt {j}"), va.lanes_lt(vb).select(va, vb));
                f32s(format!("{i} eq {j}"), va.lanes_eq(vb).select(va, vb));
                f32s(format!("{i} keep {j}"), va.lanes_gt(vb).keep(va));
                f32s(
                    format!("{i} and {j}"),
                    va.lanes_gt(vb).and(vc.lanes_lt(va)).select(va, vc),
                );
                f32s(
                    format!("{i} or {j}"),
                    va.lanes_gt(vb).or(vc.lanes_lt(va)).select(va, vc),
                );
                for (name, mask) in [
                    ("gt", va.lanes_gt(vb)),
                    ("lt", va.lanes_lt(vb)),
                    ("eq", va.lanes_eq(vb)),
                ] {
                    scalars.push((format!("{i} {name} {j} bits"), f64::from(mask.to_bitmask())));
                }

                let (wa, wb, wc) = (va.widen().high, vb.widen().low, vc.widen().high);
                f64s.push((format!("{i} + {j} f64"), wa + wb));
                f64s.push((format!("{i} - {j} f64"), wa - wb));
                f64s.push((format!("{i} * {j} f64"), wa * wb));
                f64s.push((format!("{i} / {j} f64"), wa / wb));
                f64s.push((format!("{i} mul_add {j} f64"), wa.mul_add(wb, wc)));
                f64s.push((format!("{i} max {j} f64"), wa.max(wb)));
                f64s.push((format!("{i} min {j} f64"), wa.min(wb)));
            }
        }

        let concatenated: Vec<f32> = ROWS.concat();
        for start in [0, 3, concatenated.len() - F32_LANES] {
            f32s(
                format!("load at {start}"),
                isa.load_f32_at(&concatenated, start),
            );
        }
        let pixels: [u8; 128] = std::array::from_fn(|i| (i * 7 + i / 3) as u8);
        let rgb: &[u8; 96] = pixels[..96].try_into().unwrap();
        let mut out3 = [0u8; 96];
        isa.drop_fourth_u8(&pixels, &mut out3);
        scalars.extend(out3.map(|byte| ("drop fourth".to_string(), f64::from(byte))));
        let mut out4 = [0u8; 128];
        isa.append_fourth_u8(rgb, 77, &mut out4);
        scalars.extend(out4.map(|byte| ("append fourth".to_string(), f64::from(byte))));
        for weights in [
            [13_933, 46_871, 4_732],
            [32_767, 0, 32_767],
            [0, 65_534, 2],
            [1, 1, 1],
        ] {
            for (name, sums) in [
                ("weighted sum3", isa.weighted_sum3_u8(rgb, weights)),
                (
                    "weighted sum3 of4",
                    isa.weighted_sum3_of4_u8(&pixels, weights),
                ),
            ] {
                let mut lanes = [0u8; 32];
                sums.store(&mut lanes);
                scalars.extend(lanes.map(|byte| (format!("{name} {weights:?}"), f64::from(byte))));
            }
        }
        let channels = [
            isa.load_u8(&BYTES),
            isa.splat_u8(9),
            isa.load_u8(&pixels[32..64].try_into().unwrap()),
        ];
        isa.store_interleaved3_u8(channels, &mut out3);
        scalars.extend(out3.map(|byte| ("rgb store".to_string(), f64::from(byte))));
        isa.store_interleaved4_u8(
            [channels[2], channels[0], isa.splat_u8(255), channels[1]],
            &mut out4,
        );
        scalars.extend(out4.map(|byte| ("rgba store".to_string(), f64::from(byte))));

        let bytes = isa.load_u8(&BYTES);
        let mut stored = [0u8; 32];
        bytes.store(&mut stored);
        scalars.extend(stored.map(|byte| ("u8 store".to_string(), f64::from(byte))));
        let words = bytes.widen();
        for (half, v) in [("low", words.low), ("high", words.high)] {
            u16s.push((format!("u8 widen {half}"), v));
        }
        let [a, b] = WORDS.map(|words| isa.load_u16(&words));
        u16s.push(("u16 add".to_string(), a + b));
        u16s.push(("u16 mul_low".to_string(), a.mul_low(b)));
        u16s.push(("u16 mul_high".to_string(), a.mul_high(b)));
        u16s.push(("u16 shift 1".to_string(), a.shift_right::<1>()));
        u16s.push(("u16 shift 8".to_string(), a.shift_right::<8>()));
        u16s.push(("u16 shift 15".to_string(), a.shift_right::<15>()));
        u16s.push(("u16 splat".to_string(), isa.splat_u16(40_000)));
        let mut narrowed = [0u8; 32];
        a.shift_right::<8>()
            .narrow_u8(b.shift_right::<8>())
            .store(&mut narrowed);
        scalars.extend(narrowed.map(|byte| ("u16 narrow_u8".to_string(), f64::from(byte))));
        let ints = a.widen();
        let rounded = isa.round_clamped(
            Halves {
                low: isa.load_f64(&ROUNDED[0]),
                high: isa.load_f64(&ROUNDED[1]),
            },
            isa.splat_f64(65535.0),
        );
        let loaded = INTS.map(|lanes| isa.load_i32(&lanes));
        let rounded_f32 = isa.round_clamped_f32(
            isa.load_f32(&[0.5, 1.5, -0.5, f32::NAN, 254.5, 255.4, 1e30, 16_777_215.0]),
            isa.splat_f32(16_777_216.0),
        );
        for (name, v) in [
            ("u16 widen low", ints.low),
            ("u16 widen high", ints.high),
            ("round", rounded),
            ("round f32", rounded_f32),
            ("loaded", loaded[0]),
            ("loaded weights", loaded[1]),
        ] {
            i32s.push((name.to_string(), v));
            f32s(format!("{name} to_f32"), v.to_f32());
        }
        u16s.push(("i32 narrow_u16".to_string(), ints.high.narrow_u16(rounded)));
        let [x, y] = loaded;
        i32s.push(("i32 add".to_string(), x + y));
        i32s.push(("i32 mul".to_string(), x * y));
        i32s.push(("i32 and".to_string(), x & y));
        i32s.push(("i32 shift 1".to_string(), x.shift_right::<1>()));
        i32s.push(("i32 shift 8".to_string(), x.shift_right::<8>()));
        i32s.push(("i32 shift 31".to_string(), x.shift_right::<31>()));
        i32s.push(("i32 splat".to_string(), isa.splat_i32(-7)));
        // Integers f32 holds exactly, at both ends of `i32`'s range and around zero.
        let end = 2.0f32.powi(31);
        let integral = [
            -end,
            -16_777_216.0,
            -1.0,
            -0.0,
            0.0,
            1.0,
            16_777_216.0,
            end - 128.0,
        ];
        i32s.push(("f32 to_i32".to_string(), isa.load_f32(&integral).to_i32()));
        let gather_idx = isa.load_i32(&GATHER_IDX);
        // Every lane's eight elements in each table, so the hardware Isas take their fast path.
        let whole_starts = isa.load_i32(&[0, 8, 4, 6, 1, 2, 3, 5]);
        let second = isa.load_f32(&ROWS[3]).lanes_gt(isa.splat_f32(0.75));
        for (name, valid) in [
            ("all", isa.splat_f32(0.0).lanes_eq(isa.splat_f32(0.0))),
            ("some", isa.load_f32(&ROWS[2]).lanes_gt(isa.splat_f32(0.15))),
            ("none", isa.splat_f32(0.0).lanes_gt(isa.splat_f32(0.0))),
        ] {
            i32s.push((
                format!("gather u32 {name}"),
                isa.gather_u32_le(&GATHER_BYTES, gather_idx, valid),
            ));
            for (starts, idx) in [("edges", gather_idx), ("whole", whole_starts)] {
                let pairs = [
                    isa.gather_pair_f32::<4>(&TABLE, idx, valid, second),
                    isa.gather_pair_f32::<3>(&TABLE, idx, second, valid),
                ];
                for (split, samples) in pairs.into_iter().enumerate() {
                    for (k, sample) in samples.into_iter().enumerate() {
                        f32s(format!("pair f32 {split} {starts} {name} {k}"), sample);
                    }
                }
                let pairs = [
                    isa.gather_pair_u16::<4>(&WORDS[0], idx, valid, second),
                    isa.gather_pair_u16::<3>(&WORDS[0], idx, second, valid),
                ];
                for (split, samples) in pairs.into_iter().enumerate() {
                    for (k, sample) in samples.into_iter().enumerate() {
                        i32s.push((format!("pair u16 {split} {starts} {name} {k}"), sample));
                    }
                }
                i32s.push((
                    format!("gather u32 {starts} {name}"),
                    isa.gather_u32_le(&GATHER_BYTES, idx, valid),
                ));
            }
            i32s.push((
                format!("gather u32 empty {name}"),
                isa.gather_u32_le(&[], gather_idx, valid),
            ));
        }
        let [c0, c1, c2] = [&ROWS[0], &ROWS[1], &ROWS[2]].map(|row| isa.load_f32(row));
        let mut rgba = [0.0f32; 32];
        isa.store_interleaved_f32::<4, 32>([c0, c1, c2, isa.splat_f32(-3.5)], &mut rgba);
        scalars.extend(rgba.map(|sample| ("rgba f32 store".to_string(), f64::from(sample))));
        let mut rgb = [0.0f32; 24];
        isa.store_interleaved_f32::<3, 24>([c2, c0, c1], &mut rgb);
        scalars.extend(rgb.map(|sample| ("rgb f32 store".to_string(), f64::from(sample))));
        let [w0, w1] = [ints.low, ints.high];
        let w2 = isa.load_i32(&[0, 65535, 1, 32768, 7, 255, 256, 40000]);
        let mut rgba = [0u16; 32];
        isa.store_interleaved_u16::<4, 32>([w2, w0, w1, isa.splat_i32(9)], &mut rgba);
        scalars.extend(rgba.map(|word| ("rgba u16 store".to_string(), f64::from(word))));
        let mut rgb = [0u16; 24];
        isa.store_interleaved_u16::<3, 24>([w1, w2, w0], &mut rgb);
        scalars.extend(rgb.map(|word| ("rgb u16 store".to_string(), f64::from(word))));
        for (divisor, values) in [
            (255.0, BYTES.map(f32::from).to_vec()),
            (65535.0, WORDS.concat().into_iter().map(f32::from).collect()),
        ] {
            let (divisor, reciprocal) = (isa.splat_f32(divisor), isa.splat_f32(1.0 / divisor));
            for chunk in values.chunks(F32_LANES) {
                f32s(
                    "div scale".to_string(),
                    isa.load_f32_partial(chunk).div_scale(divisor, reciprocal),
                );
            }
        }
        i32s.push((
            "u8 as i32".to_string(),
            isa.load_u8_as_i32(BYTES[20..28].try_into().unwrap()),
        ));
        u16s.push((
            "i32 narrow_u16 widened".to_string(),
            ints.low.narrow_u16(ints.high),
        ));
        let quads = ROUNDED.map(|lanes| isa.load_f64(&lanes));
        let (ordered, swapped) = (
            Halves {
                low: quads[0],
                high: quads[1],
            },
            Halves {
                low: quads[1],
                high: quads[0],
            },
        );
        u16s.push((
            "round u16".to_string(),
            isa.round_clamped_u16([ordered, swapped], isa.splat_f64(65535.0)),
        ));
        let mut narrowed = [0u8; 32];
        isa.round_clamped_u8([ordered, swapped, swapped, ordered], isa.splat_f64(255.0))
            .store(&mut narrowed);
        scalars.extend(narrowed.map(|byte| ("round u8".to_string(), f64::from(byte))));
        let mut narrowed = [0u8; 32];
        isa.narrow_i32_to_u8([
            words.low.widen().low,
            words.high.widen().high,
            words.low.widen().high,
            words.high.widen().low,
        ])
        .store(&mut narrowed);
        scalars.extend(narrowed.map(|byte| ("i32 narrow_u8".to_string(), f64::from(byte))));

        let integers = isa.load_f64(&[-1022.0, -1.0, 0.0, 1023.0]);
        f64s.push(("pow2i".to_string(), integers.pow2i()));
        f64s.push((
            "f64 partial load".to_string(),
            isa.load_f64_partial(&[1.0, -2.0, 3.0]),
        ));

        out.extend(f64s.into_iter().map(|(op, v)| Outcome {
            op,
            lanes: v.to_array().to_vec(),
        }));
        out.extend(u16s.into_iter().map(|(op, v): (String, S::U16)| {
            let mut lanes = [0u16; 16];
            v.store(&mut lanes);
            Outcome {
                op,
                lanes: lanes.map(f64::from).to_vec(),
            }
        }));
        out.extend(i32s.into_iter().map(|(op, v): (String, S::I32)| {
            let mut lanes = [0i32; 8];
            v.store(&mut lanes);
            Outcome {
                op,
                lanes: lanes.map(f64::from).to_vec(),
            }
        }));
        out.extend(scalars.into_iter().map(|(op, value)| Outcome {
            op,
            lanes: vec![value],
        }));
        out
    }
}

/// Each lane the same value: the same bits, or NaN on both sides. Rust, like IEEE 754, leaves a
/// NaN's sign and payload to the hardware, so they are not part of any op's result.
fn same(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

/// Every hardware tier computes what `Portable` computes, op by op and lane by lane. Together
/// with the `Portable` semantics pinned below, this is what makes a kernel's output the same on
/// every CPU.
#[test]
fn every_tier_matches_portable_lane_for_lane() {
    let reference = Portable::new().run(Battery);
    for tier in IsaTier::supported() {
        let outcomes = tier.run(Battery);
        assert_eq!(outcomes.len(), reference.len(), "{tier}");
        for (got, want) in outcomes.iter().zip(&reference) {
            assert_eq!(got.op, want.op);
            assert!(
                got.lanes.len() == want.lanes.len()
                    && got.lanes.iter().zip(&want.lanes).all(|(&g, &w)| same(g, w)),
                "{tier} {}: {:?} vs Portable {:?}",
                got.op,
                got.lanes,
                want.lanes
            );
        }
    }
}

/// `max` and `min` are one compare-swap, `a > b ? (b, a) : (a, b)`: an unordered pair or two
/// zeros leave both where they are, so `max` takes the second operand and `min` the first.
#[test]
fn max_and_min_are_a_compare_swap() {
    let isa = Portable::new();
    let pair = |a: f32, b: f32| {
        let (va, vb) = (isa.splat_f32(a), isa.splat_f32(b));
        [va.max(vb).to_array()[0], va.min(vb).to_array()[0]]
    };
    assert_eq!(pair(2.0, 1.0), [2.0, 1.0]);
    assert_eq!(pair(1.0, 2.0), [2.0, 1.0]);
    let [max, min] = pair(f32::NAN, 1.0);
    assert!(max == 1.0 && min.is_nan());
    let [max, min] = pair(1.0, f32::NAN);
    assert!(max.is_nan() && min == 1.0);
    let [max, min] = pair(-0.0, 0.0);
    assert_eq!(
        [max.to_bits(), min.to_bits()],
        [0.0f32.to_bits(), (-0.0f32).to_bits()]
    );
    let [max, min] = pair(0.0, -0.0);
    assert_eq!(
        [max.to_bits(), min.to_bits()],
        [(-0.0f32).to_bits(), 0.0f32.to_bits()]
    );
}

/// The folds pair lanes in their stated order, which these witnesses tell apart from any other.
///
/// f32: lanes `i` and `i + 4` first, `(1e8 − 1e8) + (1 + 1)` and twice `1 + 1`, so 6 exactly; a
/// left-to-right sum loses each 1 against 1e8 (whose ULP is 8) and ends at 3. f64:
/// `(1e17 − 1e17) + (1 + 1)` is 2; pairing lanes 0 and 2 instead loses the 1 against 1e17 (ULP 16)
/// and gives 0.
#[test]
fn reductions_fold_in_their_stated_order() {
    let isa = Portable::new();
    let lanes = [1e8, 1.0, 1.0, 1.0, -1e8, 1.0, 1.0, 1.0];
    assert_eq!(isa.load_f32(&lanes).reduce_sum(), 6.0);
    assert_eq!(lanes.iter().sum::<f32>(), 3.0);
    let lanes = [1e17, -1e17, 1.0, 1.0];
    assert_eq!(isa.load_f64(&lanes).reduce_sum(), 2.0);
    assert_eq!((lanes[0] + lanes[2]) + (lanes[1] + lanes[3]), 0.0);
}

/// A lookup clamps before it truncates: below the table and NaN read entry 0, past it the last
/// entry, and a fraction its floor.
#[test]
fn lookup_clamps_into_the_table() {
    let isa = Portable::new();
    let idx = isa.load_f32(&[-5.0, f32::NAN, 2.7, 15.0, 15.99, 16.0, 1e9, f32::INFINITY]);
    assert_eq!(
        isa.lookup_f32(&TABLE, idx).to_array(),
        [10.0, 10.0, 12.0, 25.0, 25.0, 25.0, 25.0, 25.0]
    );
}

#[test]
#[should_panic(expected = "a lookup table holds 1 to 2^24 entries, not 0")]
fn lookup_rejects_an_empty_table() {
    let isa = Portable::new();
    let _ = isa.lookup_f32(&[], isa.splat_f32(0.0));
}

/// A gather reads the lanes its mask sets, and 0 in every other and outside the table: lane 7's
/// mask is clear, and `i32::MIN` and `i32::MAX` lie far outside both tables. The byte
/// gather reads four bytes little-endian at offsets 0, 15 and 7, and at 16 the last three, zero
/// above them. The pair gather of sixteen reads entries 0–7 and 7–14, from 15 the last alone,
/// from −1 the first three after the one before the table, and from 16 nothing, each lane's
/// samples transposed into the eight vectors; its second mask holds lane 4 alone, so lanes 0 and
/// 3 read their first four.
#[test]
fn gather_reads_only_valid_lanes_inside_the_table() {
    let isa = Portable::new();
    let valid = isa
        .load_f32(&[1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0])
        .lanes_gt(isa.splat_f32(0.0));
    let mut lanes = [0; 8];
    isa.gather_u32_le(&GATHER_BYTES, isa.load_i32(&GATHER_IDX), valid)
        .store(&mut lanes);
    assert_eq!(
        lanes.map(i32::cast_unsigned),
        [
            0x0403_0201,
            0xF3F2_F110,
            0x00F3_F2F1,
            0,
            0x0B0A_0908,
            0,
            0,
            0
        ]
    );
    let second = isa
        .load_f32(&[0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0])
        .lanes_gt(isa.splat_f32(0.0));
    let samples = isa
        .gather_pair_f32::<4>(&TABLE, isa.load_i32(&GATHER_IDX), valid, second)
        .map(F32x8::to_array);
    let expected: [[f32; 8]; 8] = std::array::from_fn(|k| {
        let first = if k < 4 { 10.0 + k as f32 } else { 0.0 };
        let past_first = if k == 0 { 25.0 } else { 0.0 };
        let before_first = if (1..4).contains(&k) {
            9.0 + k as f32
        } else {
            0.0
        };
        [
            first,
            past_first,
            0.0,
            before_first,
            17.0 + k as f32,
            0.0,
            0.0,
            0.0,
        ]
    });
    assert_eq!(samples, expected);
}

/// The scale divide is the divide, bit for bit, for every integer of `[0, 255]` over 255 and of
/// `[0, 65535]` over 65535, on every tier: the reciprocal's product alone errs on 126 and 512 of
/// them.
#[test]
fn scale_divide_is_the_divide_for_every_byte_and_word() {
    #[derive(Debug)]
    struct Quotients(u16);

    impl Kernel for Quotients {
        type Output = usize;

        #[inline(always)]
        fn run<S: Isa>(self, isa: S) -> usize {
            let divisor = f32::from(self.0);
            let (divisors, reciprocal) = (isa.splat_f32(divisor), isa.splat_f32(1.0 / divisor));
            let values: Vec<f32> = (0..=self.0).map(f32::from).collect();
            let mut mismatches = 0;
            for chunk in values.chunks(F32_LANES) {
                let got = isa.load_f32_partial(chunk).div_scale(divisors, reciprocal);
                mismatches += chunk
                    .iter()
                    .zip(got.to_array())
                    .filter(|&(&v, got)| got.to_bits() != (v / divisor).to_bits())
                    .count();
            }
            mismatches
        }
    }

    for tier in IsaTier::supported() {
        for divisor in [255, 65535] {
            assert_eq!(tier.run(Quotients(divisor)), 0, "{tier}, {divisor}");
        }
    }
}

/// Lane `i` lands at bit `i`: lanes 2 and 7 set are `0b1000_0100`.
#[test]
fn bitmask_puts_lane_i_at_bit_i() {
    let isa = Portable::new();
    let lanes = isa.load_f32(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    assert_eq!(lanes.lanes_gt(isa.splat_f32(0.5)).to_bitmask(), 0b1000_0100);
}

/// `6 = 0.75 · 2³`, `1 = 0.5 · 2¹`, `0.1 = 0.8 · 2⁻³`; `2^n` is exact at both ends of the
/// normal range.
#[test]
fn frexp_and_pow2i_split_and_build_powers_of_two() {
    let isa = Portable::new();
    let split = isa
        .load_f32(&[6.0, 1.0, 0.1, 1.0, 1.0, 1.0, 1.0, 1.0])
        .frexp();
    assert_eq!(split.mantissa.to_array()[..3], [0.75, 0.5, 0.1 / 0.125]);
    assert_eq!(split.exponent.to_array()[..3], [3.0, 1.0, -3.0]);
    let powers = isa.load_f64(&[-1022.0, -1.0, 0.0, 1023.0]).pow2i();
    assert_eq!(
        powers.to_array(),
        [f64::MIN_POSITIVE, 0.5, 1.0, 2f64.powi(1023)]
    );
}

/// `exp` holds 1e-12 relative from −700 to 700, the whole range a Gaussian exponent `−½·q` takes
/// and well past it, on every tier: below 1e-300 it may underflow instead.
#[test]
fn exp_is_accurate_across_the_normal_range() {
    #[derive(Debug)]
    struct Exp<'a>(&'a [f64]);

    impl Kernel for Exp<'_> {
        type Output = Vec<f64>;

        #[inline(always)]
        fn run<S: Isa>(self, isa: S) -> Vec<f64> {
            let (chunks, []) = self.0.as_chunks::<F64_LANES>() else {
                unreachable!("the sweep is whole vectors")
            };
            chunks
                .iter()
                .flat_map(|chunk| isa.exp_f64(isa.load_f64(chunk)).to_array())
                .collect()
        }
    }

    let mut xs: Vec<f64> = (0..1400).map(|i| f64::from(i) * 0.5 - 700.0).collect();
    xs.extend([0.001, -0.001, 0.1, -0.1, PI, -PI, 1.0, -1.0]);
    for tier in IsaTier::supported() {
        for (&x, got) in xs.iter().zip(tier.run(Exp(&xs))) {
            let want = x.exp();
            if want < 1e-300 {
                assert!(got < 1e-290, "{tier} exp({x}) = {got}");
            } else {
                let error = (got - want).abs() / want;
                assert!(error < 1e-12, "{tier} exp({x}) = {got}, {error:e} off");
            }
        }
    }
}

/// The Isa a kernel ran on, by its type.
#[derive(Debug)]
struct IsaName;

impl Kernel for IsaName {
    type Output = &'static str;

    #[inline(always)]
    fn run<S: Isa>(self, _: S) -> &'static str {
        any::type_name::<S>()
    }
}

/// Dispatch runs on the widest tier: the last of the supported ones, which come narrowest first.
#[test]
fn dispatch_runs_on_the_widest_supported_tier() {
    let widest = IsaTier::supported()
        .last()
        .expect("Portable is always supported");
    assert_eq!(IsaName.dispatch(), widest.run(IsaName));
    assert_eq!(IsaTier::widest().run(IsaName), widest.run(IsaName));
}

/// `6 = 0.75 · 2³`, `1 = 0.5 · 2¹`, `0.1 = 0.8 · 2⁻³` and the smallest normal `0.5 · 2⁻¹⁰²¹` in
/// f64 lanes, on every tier.
#[test]
fn f64_frexp_splits_off_the_exponent() {
    #[derive(Debug)]
    struct Split;

    impl Kernel for Split {
        type Output = [[f64; F64_LANES]; 2];

        #[inline(always)]
        fn run<S: Isa>(self, isa: S) -> [[f64; F64_LANES]; 2] {
            let split = isa.load_f64(&[6.0, 1.0, 0.1, f64::MIN_POSITIVE]).frexp();
            [split.mantissa.to_array(), split.exponent.to_array()]
        }
    }

    for tier in IsaTier::supported() {
        assert_eq!(
            tier.run(Split),
            [[0.75, 0.5, 0.1 / 0.125, 0.5], [3.0, 1.0, -3.0, -1021.0]],
            "{tier}"
        );
    }
}

/// `ln` holds 4e-16 relative, a few ulp, from the smallest normal to the largest finite value and
/// across the mantissa's switch at √½, on every tier; at 1 it is exactly 0.
#[test]
fn ln_f64_is_accurate_at_every_magnitude() {
    #[derive(Debug)]
    struct Ln<'a>(&'a [f64]);

    impl Kernel for Ln<'_> {
        type Output = Vec<f64>;

        #[inline(always)]
        fn run<S: Isa>(self, isa: S) -> Vec<f64> {
            let (chunks, []) = self.0.as_chunks::<F64_LANES>() else {
                unreachable!("the sweep is whole vectors")
            };
            chunks
                .iter()
                .flat_map(|chunk| isa.ln_f64(isa.load_f64(chunk)).to_array())
                .collect()
        }
    }

    let mut xs: Vec<f64> = (-1020..1020).map(|e| 1.37 * 2f64.powi(e)).collect();
    xs.extend((0..1000).map(|i| 0.5 + f64::from(i) * 5e-4));
    xs.extend([
        f64::MIN_POSITIVE,
        f64::MAX,
        std::f64::consts::FRAC_1_SQRT_2,
        std::f64::consts::FRAC_1_SQRT_2.next_up(),
        std::f64::consts::FRAC_1_SQRT_2.next_down(),
        1.0 + 1e-12,
        1.0 - 1e-12,
        PI,
    ]);
    xs.resize(xs.len().next_multiple_of(F64_LANES), 1.0);
    for tier in IsaTier::supported() {
        for (&x, got) in xs.iter().zip(tier.run(Ln(&xs))) {
            let want = x.ln();
            if want == 0.0 {
                assert_eq!(got, 0.0, "{tier} ln({x})");
            } else {
                let error = (got - want).abs() / want.abs();
                assert!(error < 4e-16, "{tier} ln({x}) = {got}, {error:e} off");
            }
        }
    }
}

/// The integer ops on `Portable`, the model: a widen zero-extends, a narrow keeps each lane in its
/// range, `mul_high` keeps the high half of the 32-bit product, and a round clamps and breaks a
/// tie to the even neighbour.
#[test]
fn integer_ops_widen_narrow_and_round_as_specified() {
    fn words<V: U16x16>(v: V) -> [u16; 16] {
        let mut lanes = [0; 16];
        v.store(&mut lanes);
        lanes
    }
    fn ints<V: I32x8>(v: V) -> [i32; 8] {
        let mut lanes = [0; 8];
        v.store(&mut lanes);
        lanes
    }
    fn bytes<V: U8x32>(v: V) -> [u8; 32] {
        let mut lanes = [0; 32];
        v.store(&mut lanes);
        lanes
    }
    let isa = Portable::new();
    let mut input = [0u8; 32];
    input[0] = 255;
    input[17] = 7;
    let widened = isa.load_u8(&input).widen();
    assert_eq!([words(widened.low)[0], words(widened.high)[1]], [255, 7]);

    let mut lanes = [0u16; 16];
    lanes[..4].copy_from_slice(&[255, 1, 128, 7]);
    let narrowed = bytes(isa.load_u16(&lanes).narrow_u8(isa.splat_u16(200)));
    assert_eq!(narrowed[..5], [255, 1, 128, 7, 0]);
    assert_eq!(narrowed[16..], [200; 16]);
    // 65535 · 65281 = 0xFF00_00FF: the high half is 0xFF00, the low 0x00FF.
    let (max, factor) = (isa.splat_u16(65535), isa.splat_u16(65281));
    assert_eq!(words(max.mul_high(factor))[0], 0xFF00);
    assert_eq!(words(max.mul_low(factor))[0], 0x00FF);
    assert_eq!(words(max + isa.splat_u16(2))[0], 1);

    let rounded = isa.round_clamped(
        Halves {
            low: isa.load_f64(&[0.5, 1.5, 2.5, -0.5]),
            high: isa.load_f64(&[f64::NAN, 254.5, 255.4, 1e30]),
        },
        isa.splat_f64(255.0),
    );
    assert_eq!(ints(rounded), [0, 2, 2, 0, 0, 254, 255, 255]);
    let quads = Halves {
        low: isa.load_f64(&[0.5, 1.5, 2.5, -0.5]),
        high: isa.load_f64(&[f64::NAN, 254.5, 255.4, 1e30]),
    };
    let reversed = Halves {
        low: quads.high,
        high: quads.low,
    };
    assert_eq!(
        words(isa.round_clamped_u16([quads, reversed], isa.splat_f64(255.0))),
        [0, 2, 2, 0, 0, 254, 255, 255, 0, 254, 255, 255, 0, 2, 2, 0]
    );
    assert_eq!(
        bytes(isa.round_clamped_u8([reversed, quads, quads, reversed], isa.splat_f64(255.0)))[..16],
        [0, 254, 255, 255, 0, 2, 2, 0, 0, 2, 2, 0, 0, 254, 255, 255]
    );
    let mixed = isa.load_i32(&[2, 70, 40_000, 3, 0, 1, 65535, 255]);
    assert_eq!(
        words(mixed.narrow_u16(mixed))[..8],
        [2, 70, 40_000, 3, 0, 1, 65535, 255]
    );
    // 2²⁴ + 1 is no f32: it rounds to the even 2²⁴.
    let big = isa.load_i32(&[16_777_217; 8]);
    assert_eq!(big.to_f32().to_array()[0], 16_777_216.0);
}

/// The layout ops on `Portable`, the model: a dropped fourth byte leaves each pixel's first three,
/// an appended one follows them, and a weighted sum rounds out of Q16 with ties to even.
#[test]
fn layout_ops_move_and_weigh_bytes_as_specified() {
    let isa = Portable::new();
    let pixels: [u8; 128] = std::array::from_fn(|i| i as u8);
    let mut rgb = [0u8; 96];
    isa.drop_fourth_u8(&pixels, &mut rgb);
    assert_eq!(rgb[..6], [0, 1, 2, 4, 5, 6]);
    let mut rgba = [0u8; 128];
    isa.append_fourth_u8(&rgb, 255, &mut rgba);
    assert_eq!(rgba[..8], [0, 1, 2, 255, 4, 5, 6, 255]);
    let mut channels = [0u8; 96];
    isa.store_interleaved3_u8(
        [isa.splat_u8(1), isa.splat_u8(2), isa.splat_u8(3)],
        &mut channels,
    );
    assert_eq!(channels[..6], [1, 2, 3, 1, 2, 3]);
    let mut halves = [0u8; 96];
    halves[..3].copy_from_slice(&[255, 0, 0]);
    halves[3..6].copy_from_slice(&[1, 0, 1]);
    halves[6..9].copy_from_slice(&[3, 0, 3]);
    let mut sums = [0u8; 32];
    isa.weighted_sum3_u8(&halves, [32_767, 0, 1])
        .store(&mut sums);
    // 255 · 32767 = 8 355 585, over 2¹⁶ 127.496 → 127. 1 · 32767 + 1 = 2¹⁵ is ½ exactly, a tie
    // to the even 0; 3 · 32767 + 3 = 3 · 2¹⁵ is 1.5, a tie to the even 2.
    assert_eq!(sums[..3], [127, 0, 2]);
}

/// A weight past the weighted sum's bounds breaks its contract, which the default asserts.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "past the weighted sum's bounds")]
fn a_weight_past_its_bound_is_refused() {
    let _ = Portable::new().weighted_sum3_u8(&[0; 96], [32_768, 0, 0]);
}
