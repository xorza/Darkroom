//! `floor` as the CPU's own rounding instruction: SSE4.1's `roundss`, or arm64's `frintm`.

#[inline]
pub(super) const fn floor_f64(value: f64) -> i32 {
    value.floor() as i32
}

#[inline]
pub(super) const fn floor_f32(value: f32) -> i32 {
    value.floor() as i32
}
