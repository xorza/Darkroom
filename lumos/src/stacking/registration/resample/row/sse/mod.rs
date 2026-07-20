//! SSE4.1 and AVX2 SIMD implementations for interpolation.

#![allow(clippy::needless_range_loop)]

use glam::{DVec2, Vec2};
use imaginarium::Buffer2;
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

use crate::stacking::registration::resample::kernel;
use crate::stacking::registration::transform::Transform;

#[cfg(test)]
mod tests;

/// Warp a row using AVX2 SIMD with bilinear interpolation.
///
/// Processes 8 output pixels at a time.
///
/// # Safety
/// - Caller must ensure AVX2 is available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn bilinear_avx2(
    input: &Buffer2<f32>,
    output_row: &mut [f32],
    output_y: usize,
    transform: &Transform,
) {
    let pixels = input.pixels();
    unsafe {
        let output_width = output_row.len();
        let y = output_y as f64;

        // Extract transform coefficients
        let t = transform.matrix();
        let a = t[0] as f32;
        let b = t[1] as f32;
        let c = t[2] as f32;
        let d = t[3] as f32;
        let e = t[4] as f32;
        let f = t[5] as f32;
        let g = t[6] as f32;
        let h = t[7] as f32;

        // Pre-compute y-dependent terms
        let by_c = b * y as f32 + c;
        let ey_f = e * y as f32 + f;
        let hy_1 = h * y as f32 + 1.0;

        let a_vec = _mm256_set1_ps(a);
        let d_vec = _mm256_set1_ps(d);
        let g_vec = _mm256_set1_ps(g);
        let by_c_vec = _mm256_set1_ps(by_c);
        let ey_f_vec = _mm256_set1_ps(ey_f);
        let hy_1_vec = _mm256_set1_ps(hy_1);

        let center_min = _mm256_setzero_ps();
        let center_max_x = _mm256_set1_ps(input.width() as f32 - 1.0);
        let center_max_y = _mm256_set1_ps(input.height() as f32 - 1.0);
        let footprint_min = _mm256_set1_ps(-0.5);
        let footprint_max_x = _mm256_set1_ps(input.width() as f32 - 0.5);
        let footprint_max_y = _mm256_set1_ps(input.height() as f32 - 0.5);

        let chunks = output_width / 8;

        for chunk in 0..chunks {
            let base_x = chunk * 8;

            // Load x coordinates: [base_x, base_x+1, ..., base_x+7]
            let x_coords = _mm256_set_ps(
                (base_x + 7) as f32,
                (base_x + 6) as f32,
                (base_x + 5) as f32,
                (base_x + 4) as f32,
                (base_x + 3) as f32,
                (base_x + 2) as f32,
                (base_x + 1) as f32,
                base_x as f32,
            );

            // Compute source coordinates:
            // src_x = (a*x + by_c) / (g*x + hy_1)
            // src_y = (d*x + ey_f) / (g*x + hy_1)
            let ax = _mm256_mul_ps(a_vec, x_coords);
            let dx = _mm256_mul_ps(d_vec, x_coords);
            let gx = _mm256_mul_ps(g_vec, x_coords);

            let num_x = _mm256_add_ps(ax, by_c_vec);
            let num_y = _mm256_add_ps(dx, ey_f_vec);
            let denom = _mm256_add_ps(gx, hy_1_vec);

            let src_x = _mm256_div_ps(num_x, denom);
            let src_y = _mm256_div_ps(num_y, denom);
            let sample_x = _mm256_min_ps(_mm256_max_ps(src_x, center_min), center_max_x);
            let sample_y = _mm256_min_ps(_mm256_max_ps(src_y, center_min), center_max_y);

            // Compute integer coordinates
            let x0 = _mm256_floor_ps(sample_x);
            let y0 = _mm256_floor_ps(sample_y);
            // Compute fractional parts
            let fx = _mm256_sub_ps(sample_x, x0);
            let fy = _mm256_sub_ps(sample_y, y0);

            // Sample the four corners (scalar fallback for gather since it's complex)
            let mut p00 = [0.0f32; 8];
            let mut p10 = [0.0f32; 8];
            let mut p01 = [0.0f32; 8];
            let mut p11 = [0.0f32; 8];

            let mut x0_arr = [0.0f32; 8];
            let mut y0_arr = [0.0f32; 8];
            _mm256_storeu_ps(x0_arr.as_mut_ptr(), x0);
            _mm256_storeu_ps(y0_arr.as_mut_ptr(), y0);

            for i in 0..8 {
                let x0 = x0_arr[i] as usize;
                let y0 = y0_arr[i] as usize;
                let x1 = (x0 + 1).min(input.width() - 1);
                let y1 = (y0 + 1).min(input.height() - 1);

                p00[i] = pixels[y0 * input.width() + x0];
                p10[i] = pixels[y0 * input.width() + x1];
                p01[i] = pixels[y1 * input.width() + x0];
                p11[i] = pixels[y1 * input.width() + x1];
            }

            let p00_vec = _mm256_loadu_ps(p00.as_ptr());
            let p10_vec = _mm256_loadu_ps(p10.as_ptr());
            let p01_vec = _mm256_loadu_ps(p01.as_ptr());
            let p11_vec = _mm256_loadu_ps(p11.as_ptr());

            // Bilinear interpolation:
            // top = p00 + fx * (p10 - p00)
            // bottom = p01 + fx * (p11 - p01)
            // result = top + fy * (bottom - top)
            let top = _mm256_add_ps(p00_vec, _mm256_mul_ps(fx, _mm256_sub_ps(p10_vec, p00_vec)));
            let bottom = _mm256_add_ps(p01_vec, _mm256_mul_ps(fx, _mm256_sub_ps(p11_vec, p01_vec)));
            let result = _mm256_add_ps(top, _mm256_mul_ps(fy, _mm256_sub_ps(bottom, top)));
            let x_in = _mm256_and_ps(
                _mm256_cmp_ps(src_x, footprint_min, _CMP_GE_OQ),
                _mm256_cmp_ps(src_x, footprint_max_x, _CMP_LE_OQ),
            );
            let y_in = _mm256_and_ps(
                _mm256_cmp_ps(src_y, footprint_min, _CMP_GE_OQ),
                _mm256_cmp_ps(src_y, footprint_max_y, _CMP_LE_OQ),
            );
            let result = _mm256_and_ps(result, _mm256_and_ps(x_in, y_in));

            // Store result
            _mm256_storeu_ps(output_row.as_mut_ptr().add(base_x), result);
        }

        // Handle remainder with scalar
        let remainder_start = chunks * 8;
        for x in remainder_start..output_width {
            let src = transform.apply(DVec2::new(x as f64, y));
            output_row[x] =
                kernel::bilinear_sample(input, Vec2::new(src.x as f32, src.y as f32), 0.0);
        }
    }
}

/// Warp a row using SSE4.1 SIMD with bilinear interpolation.
///
/// Processes 4 output pixels at a time.
///
/// # Safety
/// - Caller must ensure SSE4.1 is available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
pub(crate) unsafe fn bilinear_sse(
    input: &Buffer2<f32>,
    output_row: &mut [f32],
    output_y: usize,
    transform: &Transform,
) {
    let pixels = input.pixels();
    unsafe {
        let output_width = output_row.len();
        let y = output_y as f64;

        // Extract transform coefficients
        let t = transform.matrix();
        let a = t[0] as f32;
        let b = t[1] as f32;
        let c = t[2] as f32;
        let d = t[3] as f32;
        let e = t[4] as f32;
        let f = t[5] as f32;
        let g = t[6] as f32;
        let h = t[7] as f32;

        // Pre-compute y-dependent terms
        let by_c = b * y as f32 + c;
        let ey_f = e * y as f32 + f;
        let hy_1 = h * y as f32 + 1.0;

        let a_vec = _mm_set1_ps(a);
        let d_vec = _mm_set1_ps(d);
        let g_vec = _mm_set1_ps(g);
        let by_c_vec = _mm_set1_ps(by_c);
        let ey_f_vec = _mm_set1_ps(ey_f);
        let hy_1_vec = _mm_set1_ps(hy_1);
        let center_min = _mm_setzero_ps();
        let center_max_x = _mm_set1_ps(input.width() as f32 - 1.0);
        let center_max_y = _mm_set1_ps(input.height() as f32 - 1.0);
        let footprint_min = _mm_set1_ps(-0.5);
        let footprint_max_x = _mm_set1_ps(input.width() as f32 - 0.5);
        let footprint_max_y = _mm_set1_ps(input.height() as f32 - 0.5);

        let chunks = output_width / 4;

        for chunk in 0..chunks {
            let base_x = chunk * 4;

            // Load x coordinates
            let x_coords = _mm_set_ps(
                (base_x + 3) as f32,
                (base_x + 2) as f32,
                (base_x + 1) as f32,
                base_x as f32,
            );

            // Compute source coordinates
            let ax = _mm_mul_ps(a_vec, x_coords);
            let dx = _mm_mul_ps(d_vec, x_coords);
            let gx = _mm_mul_ps(g_vec, x_coords);

            let num_x = _mm_add_ps(ax, by_c_vec);
            let num_y = _mm_add_ps(dx, ey_f_vec);
            let denom = _mm_add_ps(gx, hy_1_vec);

            let src_x = _mm_div_ps(num_x, denom);
            let src_y = _mm_div_ps(num_y, denom);
            let sample_x = _mm_min_ps(_mm_max_ps(src_x, center_min), center_max_x);
            let sample_y = _mm_min_ps(_mm_max_ps(src_y, center_min), center_max_y);

            // Compute integer coordinates
            let x0 = _mm_floor_ps(sample_x);
            let y0 = _mm_floor_ps(sample_y);

            // Compute fractional parts
            let fx = _mm_sub_ps(sample_x, x0);
            let fy = _mm_sub_ps(sample_y, y0);

            // Sample the four corners (scalar)
            let mut p00 = [0.0f32; 4];
            let mut p10 = [0.0f32; 4];
            let mut p01 = [0.0f32; 4];
            let mut p11 = [0.0f32; 4];

            let mut x0_arr = [0.0f32; 4];
            let mut y0_arr = [0.0f32; 4];

            _mm_storeu_ps(x0_arr.as_mut_ptr(), x0);
            _mm_storeu_ps(y0_arr.as_mut_ptr(), y0);

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

            let p00_vec = _mm_loadu_ps(p00.as_ptr());
            let p10_vec = _mm_loadu_ps(p10.as_ptr());
            let p01_vec = _mm_loadu_ps(p01.as_ptr());
            let p11_vec = _mm_loadu_ps(p11.as_ptr());

            // Bilinear interpolation
            let top = _mm_add_ps(p00_vec, _mm_mul_ps(fx, _mm_sub_ps(p10_vec, p00_vec)));
            let bottom = _mm_add_ps(p01_vec, _mm_mul_ps(fx, _mm_sub_ps(p11_vec, p01_vec)));
            let result = _mm_add_ps(top, _mm_mul_ps(fy, _mm_sub_ps(bottom, top)));
            let x_in = _mm_and_ps(
                _mm_cmpge_ps(src_x, footprint_min),
                _mm_cmple_ps(src_x, footprint_max_x),
            );
            let y_in = _mm_and_ps(
                _mm_cmpge_ps(src_y, footprint_min),
                _mm_cmple_ps(src_y, footprint_max_y),
            );
            let result = _mm_and_ps(result, _mm_and_ps(x_in, y_in));

            // Store result
            _mm_storeu_ps(output_row.as_mut_ptr().add(base_x), result);
        }

        // Handle remainder with scalar
        let remainder_start = chunks * 4;
        for x in remainder_start..output_width {
            let src = transform.apply(DVec2::new(x as f64, y));
            output_row[x] =
                kernel::bilinear_sample(input, Vec2::new(src.x as f32, src.y as f32), 0.0);
        }
    }
}

/// Compute the Lanczos SIZE×SIZE weighted sum for a single pixel using SSE FMA.
///
/// Generic over kernel size: Lanczos2 (SIZE=4), Lanczos3 (SIZE=6), Lanczos4 (SIZE=8).
/// - SIZE=4: single `__m128` (4 weights), one 128-bit load per row
/// - SIZE=6: one `__m256` (6 weights + 2 zero-padded lanes), one 256-bit load per row
/// - SIZE=8: one `__m256` (8 weights), one 256-bit load per row
/// # Safety
/// - Caller must ensure FMA is available.
/// - The SIZE×SIZE pixel window at `(kx, ky)` must be fully in bounds.
/// - For SIZE > 4: `kx + 7 < input_width` (reads 8 floats per row).
/// - For SIZE = 4: `kx + 3 < input_width` (reads 4 floats per row).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe fn lanczos_kernel_fma<const SIZE: usize>(
    pixels: &[f32],
    input_width: usize,
    kx: usize,
    ky: usize,
    wx: &[f32; SIZE],
    wy: &[f32; SIZE],
) -> f32 {
    // SIZE > 4 (Lanczos3=6, Lanczos4=8): one 256-bit (8-wide) load + accumulate per row. SIZE=6
    // zero-pads the top 2 lanes of wx, so their products contribute 0 to every accumulator. SIZE=4
    // stays 128-bit below — 256-bit would waste half the register. `SIZE > 4` is const, so the dead
    // branch is eliminated.
    if SIZE > 4 {
        let wx_lo = _mm_loadu_ps(wx.as_ptr());
        let wx_hi = if SIZE == 8 {
            _mm_loadu_ps(wx.as_ptr().add(4))
        } else {
            _mm_setr_ps(wx[4], wx[5], 0.0, 0.0)
        };
        let wx256 = _mm256_insertf128_ps(_mm256_castps128_ps256(wx_lo), wx_hi, 1);

        let mut acc = _mm256_setzero_ps();

        for j in 0..SIZE {
            let row_ptr = pixels.as_ptr().add((ky + j) * input_width + kx);
            let src = _mm256_loadu_ps(row_ptr);
            let wyj = _mm256_set1_ps(wy[j]);

            let sx = _mm256_mul_ps(src, wx256);
            acc = _mm256_fmadd_ps(sx, wyj, acc);
        }

        return hsum256_ps(acc);
    }

    // SIZE = 4 (Lanczos2): single 128-bit load + accumulate per row.
    let wx_lo = _mm_loadu_ps(wx.as_ptr());
    let mut acc = _mm_setzero_ps();

    for j in 0..SIZE {
        let row_ptr = pixels.as_ptr().add((ky + j) * input_width + kx);
        let src = _mm_loadu_ps(row_ptr);
        let wyj = _mm_set1_ps(wy[j]);

        let sx = _mm_mul_ps(src, wx_lo);
        acc = _mm_fmadd_ps(sx, wyj, acc);
    }

    hsum_ps(acc)
}

/// Horizontal sum of 8 floats in an AVX register.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn hsum256_ps(v: __m256) -> f32 {
    let lo = _mm256_castps256_ps128(v);
    let hi = _mm256_extractf128_ps::<1>(v);
    hsum_ps(_mm_add_ps(lo, hi))
}

/// Compute the `SIZE` separable Lanczos tap weights for fractional offset `frac` via one SIMD
/// distance/index calc + a single `i32gather` from the LUT, replacing `SIZE` scalar `lookup_positive`
/// calls. `base`/`sign` encode the per-tap distance affine `dist[i] = base[i] + sign[i]·frac` (lanes
/// `SIZE..8` are zeroed → index 0, an in-bounds dummy gather). Bit-exact with `lookup_positive`:
/// `cvtt(dist·RES + 0.5)` truncates exactly as `(x·RES + 0.5) as usize`.
///
/// # Safety
/// AVX2 must be available. `lut_values` must point to a LUT with `≥ A·RES + 1` entries (so every
/// real lane's index `∈ [0, A·RES]` is in bounds).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe fn lanczos_weights_gather<const SIZE: usize>(
    lut_values: *const f32,
    base: &[f32; 8],
    sign: &[f32; 8],
    resolution: f32,
    frac: f32,
) -> [f32; SIZE] {
    let base_v = _mm256_loadu_ps(base.as_ptr());
    let sign_v = _mm256_loadu_ps(sign.as_ptr());
    let dist = _mm256_fmadd_ps(sign_v, _mm256_set1_ps(frac), base_v);
    let scaled = _mm256_fmadd_ps(dist, _mm256_set1_ps(resolution), _mm256_set1_ps(0.5));
    let idx = _mm256_cvttps_epi32(scaled);
    let gathered = _mm256_i32gather_ps::<4>(lut_values, idx);

    let mut tmp = [0.0f32; 8];
    _mm256_storeu_ps(tmp.as_mut_ptr(), gathered);
    let mut out = [0.0f32; SIZE];
    out.copy_from_slice(&tmp[..SIZE]);
    out
}

/// Horizontal sum of 4 floats in an SSE register.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn hsum_ps(v: __m128) -> f32 {
    let shuf = _mm_movehdup_ps(v);
    let sums = _mm_add_ps(v, shuf);
    let high = _mm_movehl_ps(sums, sums);
    let total = _mm_add_ss(sums, high);
    _mm_cvtss_f32(total)
}
