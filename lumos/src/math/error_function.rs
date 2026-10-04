//! The error function and its complement, after fdlibm's `s_erf.c` (Sun Microsystems, 1993), the
//! source of musl's and most C libraries' `erf`.
//!
//! Each range has its own rational approximation: `x + x·P/Q` in `x²` near 0, `erx + P/Q` in
//! `|x| − 1` about 1, and `exp(−x² − 0.5625 + R/S)/x` in `1/x²` beyond 1.25, with `x²` split so
//! its rounding stays out of the exponential. fdlibm states an error under 1 ulp for `erf`.
#![expect(
    clippy::excessive_precision,
    reason = "fdlibm's coefficients as it publishes them, so each can be checked against the source"
)]

/// `erf(1)` rounded to 32 bits, the anchor of the range about 1.
const ERX: f64 = 8.450_629_115_104_675_292_97e-1;
/// `2/√π − 1`, for the smallest arguments.
const EFX: f64 = 1.283_791_670_955_125_863_16e-1;

/// `erf(x) = x + x·P(x²)/Q(x²)` on `|x| < 0.84375`.
const PP: [f64; 5] = [
    1.283_791_670_955_125_585_61e-1,
    -3.250_421_072_470_014_993_70e-1,
    -2.848_174_957_559_851_047_66e-2,
    -5.770_270_296_489_441_591_57e-3,
    -2.376_301_665_665_016_260_84e-5,
];
const QQ: [f64; 5] = [
    3.979_172_239_591_553_528_19e-1,
    6.502_224_998_876_729_444_85e-2,
    5.081_306_281_875_765_627_76e-3,
    1.324_947_380_043_216_445_26e-4,
    -3.960_228_278_775_368_123_20e-6,
];

/// `erf(x) = erx + P(s)/Q(s)`, `s = |x| − 1`, on `0.84375 ≤ |x| < 1.25`.
const PA: [f64; 7] = [
    -2.362_118_560_752_659_440_77e-3,
    4.148_561_186_837_483_316_66e-1,
    -3.722_078_760_357_013_238_47e-1,
    3.183_466_199_011_617_536_74e-1,
    -1.108_946_942_823_966_774_76e-1,
    3.547_830_432_561_823_593_71e-2,
    -2.166_375_594_868_790_843_00e-3,
];
const QA: [f64; 6] = [
    1.064_208_804_008_442_282_86e-1,
    5.403_979_177_021_710_489_37e-1,
    7.182_865_441_419_626_628_68e-2,
    1.261_712_198_087_616_421_12e-1,
    1.363_708_391_202_905_073_62e-2,
    1.198_449_984_679_910_741_70e-2,
];

/// `erfc(x)·x = exp(−x² − 0.5625 + R(s)/S(s))`, `s = 1/x²`, on `1.25 ≤ |x| < 1/0.35`.
const RA: [f64; 8] = [
    -9.864_944_034_847_148_227_05e-3,
    -6.938_585_727_071_817_643_72e-1,
    -1.055_862_622_532_329_098_14e1,
    -6.237_533_245_032_600_603_96e1,
    -1.623_966_694_625_734_703_55e2,
    -1.846_050_929_067_110_359_94e2,
    -8.128_743_550_630_659_342_46e1,
    -9.814_329_344_169_145_485_92,
];
const SA: [f64; 8] = [
    1.965_127_166_743_925_712_92e1,
    1.376_577_541_435_190_426_00e2,
    4.345_658_774_752_292_288_21e2,
    6.453_872_717_332_678_803_36e2,
    4.290_081_400_275_678_333_86e2,
    1.086_350_055_417_794_351_34e2,
    6.570_249_770_319_281_701_35,
    -6.042_441_521_485_809_874_38e-2,
];

/// The same on `|x| ≥ 1/0.35`.
const RB: [f64; 7] = [
    -9.864_942_924_700_099_285_97e-3,
    -7.992_832_376_805_230_065_74e-1,
    -1.775_795_491_775_475_198_89e1,
    -1.606_363_848_558_219_160_62e2,
    -6.375_664_433_683_896_277_22e2,
    -1.025_095_131_611_077_249_54e3,
    -4.835_191_916_086_513_970_19e2,
];
const SB: [f64; 7] = [
    3.033_806_074_348_245_829_24e1,
    3.257_925_129_965_739_188_26e2,
    1.536_729_586_084_436_959_94e3,
    3.199_858_219_508_595_539_08e3,
    2.553_050_406_433_164_425_83e3,
    4.745_285_412_069_553_672_15e2,
    -2.244_095_244_658_581_833_62e1,
];

/// `c₀ + s·(c₁ + s·(…))`, by Horner.
fn polynomial(coefficients: &[f64], s: f64) -> f64 {
    coefficients
        .iter()
        .rev()
        .fold(0.0, |sum, &coefficient| coefficient + s * sum)
}

/// `1 + s·(c₀ + s·(c₁ + …))`: the denominators, whose constant term is 1.
fn denominator(coefficients: &[f64], s: f64) -> f64 {
    1.0 + s * polynomial(coefficients, s)
}

/// The high 32 bits of `x`, sign included, as fdlibm compares them.
const fn high_word(x: f64) -> i32 {
    (x.to_bits() >> 32) as i32
}

/// `x·erfc(x)` for `1.25 ≤ x < 28`: `exp(−z² − 0.5625)·exp((z − x)(z + x) + R/S)` with `z` the
/// argument's top 32 bits, so `z²` is exact and the rest of `x²` goes into the second factor.
/// `split` is the high word where the second approximation takes over; fdlibm's two functions
/// place it one unit apart.
fn tail(x: f64, split: i32) -> f64 {
    let s = 1.0 / (x * x);
    let ratio = if high_word(x) < split {
        polynomial(&RA, s) / denominator(&SA, s)
    } else {
        polynomial(&RB, s) / denominator(&SB, s)
    };
    let z = f64::from_bits(x.to_bits() & 0xffff_ffff_0000_0000);
    (-z * z - 0.5625).exp() * ((z - x) * (z + x) + ratio).exp()
}

/// The error function, `(2/√π)·∫₀ˣ e^(−t²) dt`.
pub(crate) fn erf(x: f64) -> f64 {
    let ix = high_word(x) & 0x7fff_ffff;
    if x.is_nan() {
        return x;
    }
    if ix < 0x3feb_0000 {
        if ix < 0x3e30_0000 {
            return x + EFX * x;
        }
        let z = x * x;
        return x + x * (polynomial(&PP, z) / denominator(&QQ, z));
    }
    if ix < 0x3ff4_0000 {
        let s = x.abs() - 1.0;
        let ratio = polynomial(&PA, s) / denominator(&QA, s);
        return if x >= 0.0 { ERX + ratio } else { -ERX - ratio };
    }
    if ix >= 0x4018_0000 {
        return 1f64.copysign(x);
    }
    let a = x.abs();
    let r = tail(a, 0x4006_db6e) / a;
    if x >= 0.0 { 1.0 - r } else { r - 1.0 }
}

/// The complementary error function, `1 − erf(x)`, to its own relative precision far into the
/// tail.
pub(crate) fn erfc(x: f64) -> f64 {
    let hx = high_word(x);
    let ix = hx & 0x7fff_ffff;
    if x.is_nan() {
        return x;
    }
    if ix < 0x3feb_0000 {
        if ix < 0x3c70_0000 {
            return 1.0 - x;
        }
        let z = x * x;
        let y = polynomial(&PP, z) / denominator(&QQ, z);
        if hx < 0x3fd0_0000 {
            return 1.0 - (x + x * y);
        }
        return 0.5 - (x * y + (x - 0.5));
    }
    if ix < 0x3ff4_0000 {
        let s = x.abs() - 1.0;
        let ratio = polynomial(&PA, s) / denominator(&QA, s);
        return if x >= 0.0 {
            (1.0 - ERX) - ratio
        } else {
            1.0 + (ERX + ratio)
        };
    }
    if ix < 0x403c_0000 {
        if x < 0.0 && ix >= 0x4018_0000 {
            return 2.0;
        }
        let a = x.abs();
        let r = tail(a, 0x4006_db6d) / a;
        return if x > 0.0 { r } else { 2.0 - r };
    }
    if x > 0.0 { 0.0 } else { 2.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(x, erf(x), erfc(x))` rounded from 110-digit sums: the Taylor series of `erf` below 6,
    /// with `erfc = 1 − erf`, and Lentz's continued fraction of `erfc` beyond. Every range of the
    /// approximation, and its edges, is sampled.
    const REFERENCE: [(f64, f64, f64); 12] = [
        (1e-10, 1.128_379_167_095_512_6e-10, 0.999_999_999_887_162),
        (0.1, 0.112_462_916_018_284_9, 0.887_537_083_981_715),
        (0.5, 0.520_499_877_813_046_5, 0.479_500_122_186_953_5),
        (0.84375, 0.767_225_661_232_341_6, 0.232_774_338_767_658_38),
        (1.0, 0.842_700_792_949_714_9, 0.157_299_207_050_285_13),
        (1.25, 0.922_900_128_256_458_3, 0.077_099_871_743_541_77),
        (2.0, 0.995_322_265_018_952_7, 0.004_677_734_981_047_266),
        (2.857, 0.999_946_641_739_913_1, 5.335_826_008_684_637_4e-5),
        (4.0, 0.999_999_984_582_742_1, 1.541_725_790_028_002e-8),
        (5.9, 0.999_999_999_999_999_9, 7.190_409_783_550_478e-17),
        (10.0, 1.0, 2.088_487_583_762_545e-45),
        (20.0, 1.0, 5.395_865_611_607_901e-176),
    ];

    /// Within 2 ulps of the exact value: fdlibm's bound of 1 ulp, and one more for the platform's
    /// `exp`, which the tail calls. `erf` is odd exactly; `erfc(−x)` is `2 − erfc(x)` to 2 ulps.
    #[test]
    fn both_functions_match_the_reference() {
        let ulps =
            |got: f64, want: f64| (got - want).abs() / (f64::from_bits(want.to_bits() + 1) - want);
        for (x, want_erf, want_erfc) in REFERENCE {
            assert!(ulps(erf(x), want_erf) <= 2.0, "erf({x}) = {}", erf(x));
            assert!(ulps(erfc(x), want_erfc) <= 2.0, "erfc({x}) = {}", erfc(x));
            assert_eq!(erf(-x), -erf(x), "erf(−{x})");
            assert!(
                ulps(erfc(-x), 2.0 - want_erfc) <= 2.0,
                "erfc(−{x}) = {}",
                erfc(-x)
            );
        }
        assert_eq!(erf(0.0), 0.0);
        assert_eq!(erfc(0.0), 1.0);
        assert_eq!(erf(f64::INFINITY), 1.0);
        assert_eq!(erfc(30.0), 0.0);
        assert!(erf(f64::NAN).is_nan() && erfc(f64::NAN).is_nan());
    }
}
