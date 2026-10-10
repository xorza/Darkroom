use std::any;
use std::f64::consts::PI;

use crate::simd::math::Math;
use crate::simd::portable::Portable;
use crate::simd::tier::Tier;
use crate::simd::{F32_LANES, F32x8, F64_LANES, F64x4, Isa, Kernel, Mask8};

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
    for tier in Tier::supported() {
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
    let index = isa.load_f32(&[-5.0, f32::NAN, 2.7, 15.0, 15.99, 16.0, 1e9, f32::INFINITY]);
    assert_eq!(
        isa.lookup_f32(&TABLE, index).to_array(),
        [10.0, 10.0, 12.0, 25.0, 25.0, 25.0, 25.0, 25.0]
    );
}

#[test]
#[should_panic(expected = "a lookup table holds 1 to 2^24 entries, not 0")]
fn lookup_rejects_an_empty_table() {
    let isa = Portable::new();
    let _ = isa.lookup_f32(&[], isa.splat_f32(0.0));
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
    for tier in Tier::supported() {
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
    let widest = Tier::supported()
        .last()
        .expect("Portable is always supported");
    assert_eq!(IsaName.dispatch(), widest.run(IsaName));
    assert_eq!(Tier::widest().run(IsaName), widest.run(IsaName));
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

    for tier in Tier::supported() {
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
    for tier in Tier::supported() {
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
