//! AVX2/FMA implementations of the Lanczos row-warp kernels.

#![expect(
    clippy::needless_range_loop,
    reason = "indices drive pointer arithmetic over the pixel window"
)]

use std::arch::x86_64::*;

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
#[target_feature(enable = "avx2,fma")]
#[inline]
pub(super) unsafe fn lanczos_kernel_fma<const SIZE: usize>(
    pixels: &[f32],
    input_width: usize,
    kx: usize,
    ky: usize,
    wx: &[f32; SIZE],
    wy: &[f32; SIZE],
) -> f32 {
    // SAFETY: every operation below relies only on the precondition this function's own
    // safety contract already states.
    unsafe {
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
}

/// Horizontal sum of 8 floats in an AVX register.
#[target_feature(enable = "avx2,fma")]
#[inline]
unsafe fn hsum256_ps(v: __m256) -> f32 {
    // SAFETY: every operation below relies only on the precondition this function's own
    // safety contract already states.
    unsafe {
        let lo = _mm256_castps256_ps128(v);
        let hi = _mm256_extractf128_ps::<1>(v);
        hsum_ps(_mm_add_ps(lo, hi))
    }
}

/// The per-tap distance affine `dist[i] = base[i] + sign[i]·frac` that [`lanczos_weights_gather`]
/// loads. Determined by `A` and `SIZE` alone, so it is built in a `const` block rather than at
/// run time; lanes `SIZE..8` stay zero, gathering index 0 as an in-bounds dummy.
#[derive(Debug)]
struct GatherTables {
    base: [f32; 8],
    sign: [f32; 8],
}

impl GatherTables {
    /// For `i < A` the tap sits left of centre and its distance grows with `frac`; for `i ≥ A` it
    /// sits right of centre and shrinks with it, hence the sign flip.
    const fn new<const A: usize, const SIZE: usize>() -> Self {
        let mut base = [0.0f32; 8];
        let mut sign = [0.0f32; 8];
        let a_minus_1 = A as i32 - 1;
        let mut i = 0;
        while i < SIZE {
            if i < A {
                base[i] = (a_minus_1 - i as i32) as f32;
                sign[i] = 1.0;
            } else {
                base[i] = (i as i32 - a_minus_1) as f32;
                sign[i] = -1.0;
            }
            i += 1;
        }
        Self { base, sign }
    }
}

/// Compute the `SIZE` separable Lanczos tap weights for fractional offset `frac` via one SIMD
/// distance/index calc + a single `i32gather` from the LUT, replacing `SIZE` scalar `lookup_positive`
/// calls. Bit-exact with `lookup_positive`: `cvtt(dist·RES + 0.5)` truncates exactly as
/// `(x·RES + 0.5) as usize`.
///
/// # Safety
/// AVX2 must be available. `lut_values` must point to a LUT with `≥ A·RES + 1` entries (so every
/// real lane's index `∈ [0, A·RES]` is in bounds).
#[target_feature(enable = "avx2,fma")]
#[inline]
pub(super) unsafe fn lanczos_weights_gather<const A: usize, const SIZE: usize>(
    lut_values: *const f32,
    resolution: f32,
    frac: f32,
) -> [f32; SIZE] {
    // SAFETY: every operation below relies only on the precondition this function's own
    // safety contract already states.
    unsafe {
        // A `const` block, not a `let`: `#[target_feature]` bars this function from inlining into its
        // feature-less caller, so anything built here is built per call — twice per output pixel —
        // where a plain loop-invariant `let` would have been hoisted out of the row loop.
        let tables = const { GatherTables::new::<A, SIZE>() };

        let base_v = _mm256_loadu_ps(tables.base.as_ptr());
        let sign_v = _mm256_loadu_ps(tables.sign.as_ptr());
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
}

/// Horizontal sum of 4 floats in an SSE register.
#[target_feature(enable = "avx2,fma")]
#[inline]
unsafe fn hsum_ps(v: __m128) -> f32 {
    // No `unsafe` block: these shuffle/add intrinsics touch no memory, so they are safe calls
    // once `target_feature` above has established SSE — the function is `unsafe` only because
    // its callers must guarantee that.
    let shuf = _mm_movehdup_ps(v);
    let sums = _mm_add_ps(v, shuf);
    let high = _mm_movehl_ps(sums, sums);
    let total = _mm_add_ss(sums, high);
    _mm_cvtss_f32(total)
}
