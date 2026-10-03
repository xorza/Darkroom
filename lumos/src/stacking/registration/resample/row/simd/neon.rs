//! ARM NEON implementation of the Lanczos row-warp kernel.
//!
//! 128-bit (4-wide f32) counterparts of the x86 kernels in
//! [`crate::stacking::registration::resample::row::simd::x86`]. For SIZE>4 the x86 Lanczos kernel
//! is 256-bit (one `__m256`/row); NEON has no 256-bit, so it processes the same window as a 128-bit
//! lo+hi pair (`float32x4_t` + `vfmaq_f32` + horizontal `vaddvq_f32`). NEON is mandatory on
//! aarch64, so these need no runtime feature check; the caller dispatches on `cfg(target_arch)`.

#![expect(
    clippy::needless_range_loop,
    reason = "indices drive pointer arithmetic over the pixel window"
)]

use std::arch::aarch64::*;

/// NEON counterpart of
/// [`crate::stacking::registration::resample::row::simd::x86::lanczos_kernel_fma`]: separable
/// Lanczos over a `SIZE×SIZE` window. 128-bit lo+hi where the x86 kernel is 256-bit (SIZE>4).
///
/// # Safety
/// - Caller must be on aarch64.
/// - The `SIZE×SIZE` window at `(kx, ky)` must be fully in bounds. For `SIZE > 4`,
///   `kx + 7 < input_width` (reads 8 floats/row); for `SIZE = 4`, `kx + 3 < input_width`.
pub(super) unsafe fn lanczos_kernel_neon<const SIZE: usize>(
    pixels: &[f32],
    input_width: usize,
    kx: usize,
    ky: usize,
    wx: &[f32; SIZE],
    wy: &[f32; SIZE],
) -> f32 {
    unsafe {
        // Horizontal weights, constant across rows. For SIZE=6 the top half is zero-padded; the
        // dispatch guarantees the extra two columns are in bounds, and their zero weight nulls
        // them.
        let wx_lo = vld1q_f32(wx.as_ptr());
        let wx_hi = if SIZE == 8 {
            vld1q_f32(wx.as_ptr().add(4))
        } else if SIZE == 6 {
            let tmp = [wx[4], wx[5], 0.0, 0.0];
            vld1q_f32(tmp.as_ptr())
        } else {
            vdupq_n_f32(0.0)
        };

        let mut acc_lo = vdupq_n_f32(0.0);
        let mut acc_hi = vdupq_n_f32(0.0);

        for j in 0..SIZE {
            let row_ptr = pixels.as_ptr().add((ky + j) * input_width + kx);
            let src_lo = vld1q_f32(row_ptr);
            let wyj = vdupq_n_f32(wy[j]);

            let sx_lo = vmulq_f32(src_lo, wx_lo);
            acc_lo = vfmaq_f32(acc_lo, sx_lo, wyj);

            if SIZE > 4 {
                let src_hi = vld1q_f32(row_ptr.add(4));
                let sx_hi = vmulq_f32(src_hi, wx_hi);
                acc_hi = vfmaq_f32(acc_hi, sx_hi, wyj);
            }
        }

        if SIZE > 4 {
            vaddvq_f32(vaddq_f32(acc_lo, acc_hi))
        } else {
            vaddvq_f32(acc_lo)
        }
    }
}
