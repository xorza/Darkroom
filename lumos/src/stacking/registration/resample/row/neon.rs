//! ARM NEON implementations of the bilinear + Lanczos row-warp kernels.
//!
//! 128-bit (4-wide f32) counterparts of the x86 kernels in
//! [`crate::stacking::registration::resample::row::sse`]. For SIZE>4 the x86 Lanczos kernel is
//! 256-bit (one `__m256`/row); NEON has no 256-bit, so it processes the same window as a 128-bit
//! lo+hi pair (`float32x4_t` + `vfmaq_f32` + horizontal `vaddvq_f32`). NEON is mandatory on aarch64,
//! so these need no runtime feature check; the caller dispatches on `cfg(target_arch)`.

#![allow(clippy::needless_range_loop)] // indices drive pointer arithmetic over the pixel window

use std::arch::aarch64::*;

use glam::{DVec2, Vec2};
use imaginarium::Buffer2;

use crate::stacking::registration::resample::kernel;
use crate::stacking::registration::transform::Transform;

/// Warp a row using NEON bilinear interpolation, 4 output pixels at a time.
///
/// # Safety
/// Caller must be on aarch64 (NEON is always available there).
pub(crate) unsafe fn bilinear_neon(
    input: &Buffer2<f32>,
    output_row: &mut [f32],
    output_y: usize,
    transform: &Transform,
) {
    unsafe {
        let pixels = input.pixels();
        let output_width = output_row.len();
        let y = output_y as f32;

        let t = transform.matrix();
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
        let center_min = vdupq_n_f32(0.0);
        let center_max_x = vdupq_n_f32(input.width() as f32 - 1.0);
        let center_max_y = vdupq_n_f32(input.height() as f32 - 1.0);
        let footprint_min = vdupq_n_f32(-0.5);
        let footprint_max_x = vdupq_n_f32(input.width() as f32 - 0.5);
        let footprint_max_y = vdupq_n_f32(input.height() as f32 - 0.5);

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
            let sample_x = vminq_f32(vmaxq_f32(src_x, center_min), center_max_x);
            let sample_y = vminq_f32(vmaxq_f32(src_y, center_min), center_max_y);

            let x0 = vrndmq_f32(sample_x); // floor toward -inf
            let y0 = vrndmq_f32(sample_y);
            let fx = vsubq_f32(sample_x, x0);
            let fy = vsubq_f32(sample_y, y0);

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
                let x0 = x0_arr[i] as usize;
                let y0 = y0_arr[i] as usize;
                let x1 = (x0 + 1).min(input.width() - 1);
                let y1 = (y0 + 1).min(input.height() - 1);
                p00[i] = pixels[y0 * input.width() + x0];
                p10[i] = pixels[y0 * input.width() + x1];
                p01[i] = pixels[y1 * input.width() + x0];
                p11[i] = pixels[y1 * input.width() + x1];
            }
            let p00v = vld1q_f32(p00.as_ptr());
            let p10v = vld1q_f32(p10.as_ptr());
            let p01v = vld1q_f32(p01.as_ptr());
            let p11v = vld1q_f32(p11.as_ptr());

            // top = p00 + fx*(p10-p00); bottom = p01 + fx*(p11-p01); out = top + fy*(bottom-top)
            let top = vfmaq_f32(p00v, fx, vsubq_f32(p10v, p00v));
            let bottom = vfmaq_f32(p01v, fx, vsubq_f32(p11v, p01v));
            let result = vfmaq_f32(top, fy, vsubq_f32(bottom, top));
            let x_in = vandq_u32(
                vcgeq_f32(src_x, footprint_min),
                vcleq_f32(src_x, footprint_max_x),
            );
            let y_in = vandq_u32(
                vcgeq_f32(src_y, footprint_min),
                vcleq_f32(src_y, footprint_max_y),
            );
            let result = vbslq_f32(vandq_u32(x_in, y_in), result, vdupq_n_f32(0.0));

            vst1q_f32(output_row.as_mut_ptr().add(base_x), result);
        }

        // Scalar remainder.
        for x in (chunks * 4)..output_width {
            let src = transform.apply(DVec2::new(x as f64, output_y as f64));
            output_row[x] =
                kernel::bilinear_sample(input, Vec2::new(src.x as f32, src.y as f32), 0.0);
        }
    }
}

/// NEON counterpart of
/// [`crate::stacking::registration::resample::row::sse::lanczos_kernel_fma`]: separable
/// Lanczos over a `SIZE×SIZE` window. 128-bit lo+hi where the x86 kernel is 256-bit (SIZE>4).
///
/// # Safety
/// - Caller must be on aarch64.
/// - The `SIZE×SIZE` window at `(kx, ky)` must be fully in bounds. For `SIZE > 4`,
///   `kx + 7 < input_width` (reads 8 floats/row); for `SIZE = 4`, `kx + 3 < input_width`.
pub(crate) unsafe fn lanczos_kernel_neon<const SIZE: usize>(
    pixels: &[f32],
    input_width: usize,
    kx: usize,
    ky: usize,
    wx: &[f32; SIZE],
    wy: &[f32; SIZE],
) -> f32 {
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

#[cfg(test)]
mod tests {
    use crate::stacking::registration::resample::kernel;
    use crate::stacking::registration::resample::row;
    use crate::stacking::registration::resample::row::neon;
    use crate::stacking::registration::transform::{Transform, WarpTransform};
    use glam::DVec2;

    #[test]
    fn bilinear_neon_matches_scalar_at_both_footprint_edges() {
        const WIDTH: usize = 12;
        const HEIGHT: usize = 8;
        let input = Buffer2::new_filled(WIDTH, HEIGHT, 3.25);

        for translation in [-0.75, 0.75] {
            let transform = Transform::translation(DVec2::new(translation, 0.0));
            let mut neon = vec![0.0; WIDTH];
            let mut scalar = vec![0.0; WIDTH];
            unsafe {
                neon::bilinear_neon(&input, &mut neon, HEIGHT / 2, &transform);
            }
            row::bilinear_scalar(
                &input,
                &mut scalar,
                HEIGHT / 2,
                &WarpTransform::new(transform),
                0.0,
            );

            assert_eq!(neon, scalar, "translation {translation}");
            let outside_x = if translation < 0.0 { 0 } else { WIDTH - 1 };
            assert_eq!(neon[outside_x], 0.0, "translation {translation}");
        }
    }

    /// NEON Lanczos kernel must match a plain scalar weighted sum (mirror of the x86
    /// `lanczos_kernel_fma_matches_scalar` checks). Interior 20×20 window, no border.
    fn assert_lanczos_kernel_neon_matches_scalar<const A: usize, const SIZE: usize>(label: &str) {
        let (width, height) = (20usize, 20usize);
        let data: Vec<f32> = (0..width * height)
            .map(|i| (i % width) as f32 + (i / width) as f32 * 0.1)
            .collect();

        let lut = kernel::get_lanczos_lut(A);
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

        let simd = unsafe { neon::lanczos_kernel_neon::<SIZE>(&data, width, kx, ky, &wx, &wy) };
        assert!(
            (simd - scalar_sum).abs() < 1e-4,
            "{label}: NEON {simd} vs scalar {scalar_sum}",
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
