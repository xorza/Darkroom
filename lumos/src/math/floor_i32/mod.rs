//! [`FloorI32`]: `floor` to an `i32`.

/// `floor` to an `i32`, for a finite value within `i32` range: one rounding instruction on every
/// target the workspace builds, SSE4.1's `roundss` within the x86-64-v3 baseline and arm64's
/// `frintm`.
pub(crate) trait FloorI32 {
    fn floor_i32(self) -> i32;
}

impl FloorI32 for f64 {
    #[inline]
    fn floor_i32(self) -> i32 {
        self.floor() as i32
    }
}

impl FloorI32 for f32 {
    #[inline]
    fn floor_i32(self) -> i32 {
        self.floor() as i32
    }
}

#[cfg(test)]
mod tests;
