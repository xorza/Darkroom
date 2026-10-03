//! What a channel value means in each storage type, and the one rule for moving it between them.
//!
//! Every type stores a value on `[0, 1]` scaled by its full scale: 255 for `u8`, 65535 for `u16`,
//! one for `f32`. Widening divides by the full scale. Narrowing multiplies, then rounds to the
//! nearest integer with ties to even — the rule `cvtps2dq` under the default MXCSR, NEON `vcvtn`
//! and WGSL `round` all follow, so no backend has to deviate from it — and saturates to the
//! type's range, with NaN going to zero as Rust's `as` takes it.

use std::fmt::Debug;

use bytemuck::Pod;

use crate::common::color_format::SampleType;

/// A channel storage type.
pub(crate) trait Sample: Pod + Debug + PartialEq + Send + Sync + 'static {
    const TYPE: SampleType;
    /// The value that stands for one: full intensity, and an opaque alpha.
    const FULL_SCALE: Self;
    /// [`Self::FULL_SCALE`] as an `f32`.
    const FULL_SCALE_F32: f32;

    /// The stored value itself, exact for every integer of `u8` and `u16`.
    fn to_f32(self) -> f32;

    /// The value on `[0, 1]`: the correctly rounded quotient by the full scale.
    fn to_unit(self) -> f32;

    /// A value on `[0, 1]` in this type. The product with the full scale is taken in `f64`,
    /// where an `f32` times a 16-bit integer is exact, so the rounding sees the true product.
    /// `f32` storage keeps the value as it is.
    fn from_unit(unit: f32) -> Self;

    /// A value already in this type's units, narrowed by the rounding rule. `f32` storage keeps
    /// the value as it is.
    fn from_f32(value: f32) -> Self;

    /// The value in another storage type: `to_unit`, then `from_unit`. Between the integer types
    /// that is exactly `v · 257` widening and `round(v / 257)` narrowing for every input — the
    /// `f32` quotient errs by under `2⁻²⁴` relative, and no exact quotient lies that close to a
    /// rounding boundary.
    #[inline]
    fn convert<To: Sample>(self) -> To {
        To::from_unit(self.to_unit())
    }
}

#[expect(
    clippy::cast_sign_loss,
    reason = "the saturating `as` cast is the narrowing rule: negatives and NaN to zero, overflow to the maximum"
)]
impl Sample for u8 {
    const TYPE: SampleType = SampleType::U8;
    const FULL_SCALE: Self = Self::MAX;
    const FULL_SCALE_F32: f32 = 255.0;

    #[inline]
    fn to_f32(self) -> f32 {
        f32::from(self)
    }

    #[inline]
    fn to_unit(self) -> f32 {
        f32::from(self) / Self::FULL_SCALE_F32
    }

    #[inline]
    fn from_unit(unit: f32) -> Self {
        (f64::from(unit) * f64::from(Self::FULL_SCALE)).round_ties_even() as Self
    }

    #[inline]
    fn from_f32(value: f32) -> Self {
        value.round_ties_even() as Self
    }
}

#[expect(
    clippy::cast_sign_loss,
    reason = "the saturating `as` cast is the narrowing rule: negatives and NaN to zero, overflow to the maximum"
)]
impl Sample for u16 {
    const TYPE: SampleType = SampleType::U16;
    const FULL_SCALE: Self = Self::MAX;
    const FULL_SCALE_F32: f32 = 65535.0;

    #[inline]
    fn to_f32(self) -> f32 {
        f32::from(self)
    }

    #[inline]
    fn to_unit(self) -> f32 {
        f32::from(self) / Self::FULL_SCALE_F32
    }

    #[inline]
    fn from_unit(unit: f32) -> Self {
        (f64::from(unit) * f64::from(Self::FULL_SCALE)).round_ties_even() as Self
    }

    #[inline]
    fn from_f32(value: f32) -> Self {
        value.round_ties_even() as Self
    }
}

impl Sample for f32 {
    const TYPE: SampleType = SampleType::F32;
    const FULL_SCALE: Self = 1.0;
    const FULL_SCALE_F32: f32 = 1.0;

    #[inline]
    fn to_f32(self) -> f32 {
        self
    }

    #[inline]
    fn to_unit(self) -> f32 {
        self
    }

    #[inline]
    fn from_unit(unit: f32) -> Self {
        unit
    }

    #[inline]
    fn from_f32(value: f32) -> Self {
        value
    }
}

#[cfg(test)]
mod tests {
    use crate::common::sample::Sample;

    /// Exhaustive over both integer types: the one rule reduces to the integer formulas.
    #[test]
    fn integer_conversions_are_exact_scalings() {
        for v in 0..=u8::MAX {
            assert_eq!(v.convert::<u16>(), u16::from(v) * 257, "{v}");
            assert_eq!(v.convert::<u8>(), v, "{v}");
        }
        for v in 0..=u16::MAX {
            // 257 is odd, so `v / 257` is never a tie and `+ 128` rounds it.
            let nearest = u8::try_from((u32::from(v) + 128) / 257).unwrap();
            assert_eq!(v.convert::<u8>(), nearest, "{v}");
            assert_eq!(v.convert::<u16>(), v, "{v}");
            assert_eq!(v.convert::<f32>().convert::<u16>(), v, "{v}");
        }
    }

    /// `0.5 · 255 = 127.5` ties to the even 128; `0.5 · 65535 = 32767.5` ties to the even 32768.
    /// Out-of-range values saturate and NaN goes to zero.
    #[test]
    fn narrowing_rounds_ties_to_even_and_saturates() {
        for (unit, as_u8, as_u16) in [
            (0.0, 0, 0),
            (0.5, 128, 32768),
            (1.0, 255, 65535),
            (-0.25, 0, 0),
            (1.5, 255, 65535),
            (f32::INFINITY, 255, 65535),
            (f32::NEG_INFINITY, 0, 0),
            (f32::NAN, 0, 0),
        ] {
            assert_eq!(u8::from_unit(unit), as_u8, "{unit}");
            assert_eq!(u16::from_unit(unit), as_u16, "{unit}");
        }
        assert_eq!(u8::from_f32(2.5), 2);
        assert_eq!(u8::from_f32(3.5), 4);
        assert_eq!(u16::from_f32(65535.4), 65535);
        assert_eq!(u16::from_f32(-0.6), 0);
        assert_eq!(f32::from_f32(-0.6), -0.6);
        assert_eq!(f32::from_unit(1.5), 1.5);
    }

    /// The f64 product is what the rule asks for: in `f32`, `x · 255` can round onto `k + ½` for
    /// an input whose true product lies just past it, and ties-to-even then picks the wrong side.
    /// `0x3C20_A0A1` is one: its true product is `2.50000009…`, which rounds to 3, while the `f32`
    /// product is exactly 2.5, which ties to 2.
    #[test]
    fn the_product_is_exact() {
        let x = f32::from_bits(0x3C20_A0A1);
        let exact = f64::from(x) * 255.0;
        assert!(exact > 2.5 && exact < 2.500_001, "{exact}");
        assert_eq!(x * 255.0, 2.5, "the f32 product lands on the tie");
        assert_eq!(u8::from_unit(x), 3);
    }
}
