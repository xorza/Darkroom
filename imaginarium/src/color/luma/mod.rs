//! Rec. 709 luma: `Y = 0.2126 R + 0.7152 G + 0.0722 B` (ITU-R BT.709-6, item 3.2).

/// The weights as the standard states them.
const WEIGHTS: [f64; 3] = [0.2126, 0.7152, 0.0722];

/// The weights for `f32` channel values.
pub(crate) const WEIGHTS_F32: [f32; 3] = [WEIGHTS[0] as f32, WEIGHTS[1] as f32, WEIGHTS[2] as f32];

/// The weights in 16-bit fixed point for integer channel values: each the nearest multiple of
/// `2⁻¹⁶`. They sum to exactly `2¹⁶`, so a grey pixel keeps its value.
pub(crate) const WEIGHTS_Q16: [u32; 3] = {
    let q16 = [
        nearest_q16(WEIGHTS[0]),
        nearest_q16(WEIGHTS[1]),
        nearest_q16(WEIGHTS[2]),
    ];
    assert!(q16[0] + q16[1] + q16[2] == 1 << 16);
    q16
};

#[expect(clippy::cast_sign_loss, reason = "every weight is positive")]
const fn nearest_q16(weight: f64) -> u32 {
    (weight * 65536.0 + 0.5) as u32
}

/// A weighted sum in 16-bit fixed point, rounded to the nearest integer with ties to even — the
/// crate's one narrowing rule.
pub(crate) const fn round_q16(sum: u64) -> u64 {
    (sum + 0x7FFF + ((sum >> 16) & 1)) >> 16
}

#[cfg(test)]
mod tests;
