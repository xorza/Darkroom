//! [`FloorI32`]: `floor` to an `i32`, by the fastest exact method the target has.

#[cfg(not(all(target_arch = "x86_64", not(target_feature = "sse4.1"))))]
mod instruction;
#[cfg(any(test, all(target_arch = "x86_64", not(target_feature = "sse4.1"))))]
mod truncation;

#[cfg(not(all(target_arch = "x86_64", not(target_feature = "sse4.1"))))]
use crate::math::floor_i32::instruction as method;
#[cfg(all(target_arch = "x86_64", not(target_feature = "sse4.1")))]
use crate::math::floor_i32::truncation as method;

/// `floor` to an `i32`, for a finite value within `i32` range. The resamplers take it per output
/// pixel: where the target rounds in one instruction it is that instruction, and where `floor` is a
/// library call, truncation and one step down for a negative non-integer.
pub(crate) trait FloorI32 {
    fn floor_i32(self) -> i32;
}

impl FloorI32 for f64 {
    #[inline]
    fn floor_i32(self) -> i32 {
        method::floor_f64(self)
    }
}

impl FloorI32 for f32 {
    #[inline]
    fn floor_i32(self) -> i32 {
        method::floor_f32(self)
    }
}

#[cfg(feature = "bench")]
pub(crate) mod bench;

#[cfg(test)]
mod tests {
    use crate::math::floor_i32::{FloorI32, truncation};

    /// The library `floor` on both sides of 0, on integers and halves, and an ulp either side of an
    /// integer: −3.5 → −4, −3 → −3, −0.5 → −1, −0.0 → 0, 0.5 → 0, 2 → 2, 2 − ulp → 1.
    #[test]
    fn floor_i32_is_the_library_floor() {
        for x in [
            -3.5f64,
            -3.0,
            -0.5,
            -0.0,
            0.5,
            2.0,
            2.0 - f64::EPSILON,
            -1e-300,
            123_456.75,
        ] {
            assert_eq!(x.floor_i32(), x.floor() as i32, "{x}");
            let narrow = x as f32;
            assert_eq!(narrow.floor_i32(), narrow.floor() as i32, "{narrow}");
        }
        assert_eq!((-0.5f32).floor_i32(), -1);
        assert_eq!((2.0f64 - f64::EPSILON).floor_i32(), 1);
    }

    /// The truncation method agrees with the library `floor` on the same values, whichever method
    /// this target builds.
    #[test]
    fn truncation_is_the_library_floor() {
        for x in [
            -3.5f64,
            -3.0,
            -0.5,
            -0.0,
            0.5,
            2.0,
            2.0 - f64::EPSILON,
            -1e-300,
            123_456.75,
        ] {
            assert_eq!(truncation::floor_f64(x), x.floor() as i32, "{x}");
            let narrow = x as f32;
            assert_eq!(
                truncation::floor_f32(narrow),
                narrow.floor() as i32,
                "{narrow}"
            );
        }
    }
}
