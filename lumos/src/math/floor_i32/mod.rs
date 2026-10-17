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

#[cfg(feature = "internals")]
pub(crate) mod bench;

#[cfg(test)]
mod tests;
