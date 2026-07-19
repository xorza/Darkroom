//! ARM NEON implementations of the bilinear + Lanczos row-warp kernels.
//!
//! 128-bit (4-wide f32) counterparts of the x86 kernels in
//! [`crate::stacking::registration::interpolation::warp::sse`]. For SIZE>4 the x86 Lanczos kernel is
//! 256-bit (one `__m256`/row); NEON has no 256-bit, so it processes the same window as a 128-bit
//! lo+hi pair (`float32x4_t` + `vfmaq_f32` + horizontal `vaddvq_f32`). NEON is mandatory on aarch64,
//! so these need no runtime feature check; the caller dispatches on `cfg(target_arch)`.

#![allow(clippy::needless_range_loop)] // indices drive pointer arithmetic over the pixel window

use std::arch::aarch64::*;

use common::Vec2us;
use glam::{DVec2, IVec2, Vec2};
use imaginarium::Buffer2;

use crate::stacking::registration::interpolation::warp::SoftClampAccum;
use crate::stacking::registration::interpolation::{bilinear_sample, sample_pixel};
use crate::stacking::registration::transform::Transform;

/// Warp a row using NEON bilinear interpolation, 4 output pixels at a time.
///
/// # Safety
/// Caller must be on aarch64 (NEON is always available there).
pub(crate) unsafe fn warp_row_bilinear_neon(
    input: &Buffer2<f32>,
    output_row: &mut [f32],
    output_y: usize,
    transform: &Transform,
) {
    unsafe {
        let pixels = input.pixels();
        let dims = Vec2us::new(input.width(), input.height());
        let output_width = output_row.len();
        let y = output_y as f32;

        let t = transform.matrix.as_array();
        let (a, b, c) = (t[0] as f32, t[1] as f32, t[2] as f32);
        let (d, e, f) = (t[3] as f32, t[4] as f32, t[5] as f32);
        let (g, h) = (t[6] as f32, t[7] as f32);

        // src_x = (a*x + by_c) / (g*x + hy_1), src_y = (d*x + ey_f) / (g*x + hy_1)
        let by_c = vdupq_n_f32(b * y + c);
        let ey_f = vdupq_n_f32(e * y + f);
        let hy_1 = vdupq_n_f32(h * y + 1.0);
        let a_vec = vdupq_n_f32(a);
        let d_vec = vdupq_n_f32(d);
        let g_vec = vdupq_n_f32(g);

        let chunks = output_width / 4;
        for chunk in 0..chunks {
            let base_x = chunk * 4;
            let xs = [
                base_x as f32,
                (base_x + 1) as f32,
                (base_x + 2) as f32,
                (base_x + 3) as f32,
            ];
            let x_coords = vld1q_f32(xs.as_ptr());

            let num_x = vfmaq_f32(by_c, a_vec, x_coords);
            let num_y = vfmaq_f32(ey_f, d_vec, x_coords);
            let denom = vfmaq_f32(hy_1, g_vec, x_coords);
            let src_x = vdivq_f32(num_x, denom);
            let src_y = vdivq_f32(num_y, denom);

            let x0 = vrndmq_f32(src_x); // floor toward -inf
            let y0 = vrndmq_f32(src_y);
            let fx = vsubq_f32(src_x, x0);
            let fy = vsubq_f32(src_y, y0);

            // Gather the four corners scalar-ly (mirrors the AVX2 path).
            let mut x0_arr = [0.0f32; 4];
            let mut y0_arr = [0.0f32; 4];
            vst1q_f32(x0_arr.as_mut_ptr(), x0);
            vst1q_f32(y0_arr.as_mut_ptr(), y0);

            let mut p00 = [0.0f32; 4];
            let mut p10 = [0.0f32; 4];
            let mut p01 = [0.0f32; 4];
            let mut p11 = [0.0f32; 4];
            for i in 0..4 {
                let ix0 = x0_arr[i] as i32;
                let iy0 = y0_arr[i] as i32;
                p00[i] = sample_pixel(pixels, dims, IVec2::new(ix0, iy0), 0.0);
                p10[i] = sample_pixel(pixels, dims, IVec2::new(ix0 + 1, iy0), 0.0);
                p01[i] = sample_pixel(pixels, dims, IVec2::new(ix0, iy0 + 1), 0.0);
                p11[i] = sample_pixel(pixels, dims, IVec2::new(ix0 + 1, iy0 + 1), 0.0);
            }
            let p00v = vld1q_f32(p00.as_ptr());
            let p10v = vld1q_f32(p10.as_ptr());
            let p01v = vld1q_f32(p01.as_ptr());
            let p11v = vld1q_f32(p11.as_ptr());

            // top = p00 + fx*(p10-p00); bottom = p01 + fx*(p11-p01); out = top + fy*(bottom-top)
            let top = vfmaq_f32(p00v, fx, vsubq_f32(p10v, p00v));
            let bottom = vfmaq_f32(p01v, fx, vsubq_f32(p11v, p01v));
            let result = vfmaq_f32(top, fy, vsubq_f32(bottom, top));

            vst1q_f32(output_row.as_mut_ptr().add(base_x), result);
        }

        // Scalar remainder.
        for x in (chunks * 4)..output_width {
            let src = transform.apply(DVec2::new(x as f64, output_y as f64));
            output_row[x] = bilinear_sample(input, Vec2::new(src.x as f32, src.y as f32), 0.0);
        }
    }
}

/// NEON counterpart of [`crate::stacking::registration::interpolation::warp::sse::lanczos_kernel_fma`]:
/// separable Lanczos over a `SIZE×SIZE` window with optional PixInsight-style soft-clamp deringing
/// (positive/negative contributions split). 128-bit lo+hi where the x86 kernel is 256-bit (SIZE>4).
///
/// # Safety
/// - Caller must be on aarch64.
/// - The `SIZE×SIZE` window at `(kx, ky)` must be fully in bounds. For `SIZE > 4`,
///   `kx + 7 < input_width` (reads 8 floats/row); for `SIZE = 4`, `kx + 3 < input_width`.
pub(crate) unsafe fn lanczos_kernel_neon<
    const A: usize,
    const SIZE: usize,
    const DERINGING: bool,
>(
    pixels: &[f32],
    input_width: usize,
    kx: usize,
    ky: usize,
    wx: &[f32; SIZE],
    wy: &[f32; SIZE],
) -> SoftClampAccum {
    unsafe {
        // Horizontal weights, constant across rows. For SIZE=6 the top half is zero-padded; the
        // dispatch guarantees the extra two columns are in bounds, and their zero weight nulls them.
        let wx_lo = vld1q_f32(wx.as_ptr());
        let wx_hi = if SIZE == 8 {
            vld1q_f32(wx.as_ptr().add(4))
        } else if SIZE == 6 {
            let tmp = [wx[4], wx[5], 0.0, 0.0];
            vld1q_f32(tmp.as_ptr())
        } else {
            vdupq_n_f32(0.0)
        };

        let zero = vdupq_n_f32(0.0);
        let mut acc_lo = zero;
        let mut acc_hi = zero;
        let mut sp_lo = zero;
        let mut sp_hi = zero;
        let mut sn_lo = zero;
        let mut sn_hi = zero;
        let mut wp_lo = zero;
        let mut wp_hi = zero;
        let mut wn_lo = zero;
        let mut wn_hi = zero;

        for j in 0..SIZE {
            let row_ptr = pixels.as_ptr().add((ky + j) * input_width + kx);
            let src_lo = vld1q_f32(row_ptr);
            let wyj = vdupq_n_f32(wy[j]);

            if DERINGING {
                let w_lo = vmulq_f32(wx_lo, wyj);
                let s_lo = vmulq_f32(src_lo, w_lo);
                // pos lanes: s >= 0; the complement (s < 0) feeds the negative accumulators.
                let pos_lo = vcgeq_f32(s_lo, zero);
                sp_lo = vaddq_f32(sp_lo, vbslq_f32(pos_lo, s_lo, zero));
                wp_lo = vaddq_f32(wp_lo, vbslq_f32(pos_lo, w_lo, zero));
                sn_lo = vsubq_f32(sn_lo, vbslq_f32(pos_lo, zero, s_lo));
                wn_lo = vsubq_f32(wn_lo, vbslq_f32(pos_lo, zero, w_lo));

                if SIZE > 4 {
                    let src_hi = vld1q_f32(row_ptr.add(4));
                    let w_hi = vmulq_f32(wx_hi, wyj);
                    let s_hi = vmulq_f32(src_hi, w_hi);
                    let pos_hi = vcgeq_f32(s_hi, zero);
                    sp_hi = vaddq_f32(sp_hi, vbslq_f32(pos_hi, s_hi, zero));
                    wp_hi = vaddq_f32(wp_hi, vbslq_f32(pos_hi, w_hi, zero));
                    sn_hi = vsubq_f32(sn_hi, vbslq_f32(pos_hi, zero, s_hi));
                    wn_hi = vsubq_f32(wn_hi, vbslq_f32(pos_hi, zero, w_hi));
                }
            } else {
                let sx_lo = vmulq_f32(src_lo, wx_lo);
                acc_lo = vfmaq_f32(acc_lo, sx_lo, wyj);

                if SIZE > 4 {
                    let src_hi = vld1q_f32(row_ptr.add(4));
                    let sx_hi = vmulq_f32(src_hi, wx_hi);
                    acc_hi = vfmaq_f32(acc_hi, sx_hi, wyj);
                }
            }
        }

        if DERINGING {
            if SIZE > 4 {
                SoftClampAccum {
                    sp: vaddvq_f32(vaddq_f32(sp_lo, sp_hi)),
                    sn: vaddvq_f32(vaddq_f32(sn_lo, sn_hi)),
                    wp: vaddvq_f32(vaddq_f32(wp_lo, wp_hi)),
                    wn: vaddvq_f32(vaddq_f32(wn_lo, wn_hi)),
                }
            } else {
                SoftClampAccum {
                    sp: vaddvq_f32(sp_lo),
                    sn: vaddvq_f32(sn_lo),
                    wp: vaddvq_f32(wp_lo),
                    wn: vaddvq_f32(wn_lo),
                }
            }
        } else if SIZE > 4 {
            SoftClampAccum {
                sp: vaddvq_f32(vaddq_f32(acc_lo, acc_hi)),
                sn: 0.0,
                wp: 0.0,
                wn: 0.0,
            }
        } else {
            SoftClampAccum {
                sp: vaddvq_f32(acc_lo),
                sn: 0.0,
                wp: 0.0,
                wn: 0.0,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::stacking::registration::interpolation::get_lanczos_lut;
    use crate::stacking::registration::interpolation::warp::neon::*;

    /// NEON Lanczos kernel must match a plain scalar weighted sum (mirror of the x86
    /// `lanczos_kernel_fma_matches_scalar` checks). Interior 20×20 window, no border.
    fn assert_lanczos_kernel_neon_matches_scalar<const A: usize, const SIZE: usize>(label: &str) {
        let (width, height) = (20usize, 20usize);
        let data: Vec<f32> = (0..width * height)
            .map(|i| (i % width) as f32 + (i / width) as f32 * 0.1)
            .collect();

        let lut = get_lanczos_lut(A);
        let a_minus_1 = A as i32 - 1;
        let kx = (6 - a_minus_1) as usize;
        let ky = (6 - a_minus_1) as usize;
        let (fx, fy) = (0.3f32, 0.7f32);

        let mut wx = [0.0f32; SIZE];
        let mut wy = [0.0f32; SIZE];
        for i in 0..SIZE {
            wx[i] = if i < A {
                lut.lookup_positive((a_minus_1 - i as i32) as f32 + fx)
            } else {
                lut.lookup_positive((i as i32 - a_minus_1) as f32 - fx)
            };
            wy[i] = if i < A {
                lut.lookup_positive((a_minus_1 - i as i32) as f32 + fy)
            } else {
                lut.lookup_positive((i as i32 - a_minus_1) as f32 - fy)
            };
        }

        let mut scalar_sum = 0.0f32;
        for j in 0..SIZE {
            for k in 0..SIZE {
                scalar_sum += data[(ky + j) * width + kx + k] * wx[k] * wy[j];
            }
        }

        let simd = unsafe { lanczos_kernel_neon::<A, SIZE, false>(&data, width, kx, ky, &wx, &wy) };
        assert!(
            (simd.sp - scalar_sum).abs() < 1e-4,
            "{label} no-dering: NEON {} vs scalar {scalar_sum}",
            simd.sp
        );
        assert_eq!(simd.sn, 0.0);

        // With deringing, sp - sn reconstructs the same weighted sum.
        let simd_d =
            unsafe { lanczos_kernel_neon::<A, SIZE, true>(&data, width, kx, ky, &wx, &wy) };
        let total = simd_d.sp - simd_d.sn;
        assert!(
            (total - scalar_sum).abs() < 1e-4,
            "{label} dering: sp-sn={total} vs scalar {scalar_sum}"
        );
    }

    #[test]
    fn lanczos2_kernel_neon_matches_scalar() {
        assert_lanczos_kernel_neon_matches_scalar::<2, 4>("Lanczos2");
    }

    #[test]
    fn lanczos3_kernel_neon_matches_scalar() {
        assert_lanczos_kernel_neon_matches_scalar::<3, 6>("Lanczos3");
    }

    #[test]
    fn lanczos4_kernel_neon_matches_scalar() {
        assert_lanczos_kernel_neon_matches_scalar::<4, 8>("Lanczos4");
    }
}
