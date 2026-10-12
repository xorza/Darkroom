//! `floor` as truncation and one step down for a negative non-integer: where `floor` is a library
//! call, as on the x86-64 baseline, which has no rounding instruction before SSE4.1.

#[inline]
pub(super) fn floor_f64(value: f64) -> i32 {
    let truncated = value as i32;
    truncated - i32::from(value < f64::from(truncated))
}

#[inline]
pub(super) fn floor_f32(value: f32) -> i32 {
    let truncated = value as i32;
    truncated - i32::from(value < truncated as f32)
}
