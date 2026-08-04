//! AVX2 row kernels — the same shape as the SSE4.1 ones at twice the lane
//! count, which is what the op runs out of first once an image is cache
//! resident.
//!
//! The multiply and add are kept separate rather than fused into `vfmadd`:
//! fusing would round once where the scalar reference rounds twice, and the
//! integer formats are cross-checked for bit equality. That is also why these
//! ask only for `avx2`, never `fma`.

use std::arch::x86_64::*;

use crate::ops::contrast_brightness::cpu::{ChannelAffine, ContrastBrightnessApply};

/// The affine's constants, splatted once per row.
#[derive(Debug, Clone, Copy)]
struct Splat {
    scale: __m256,
    offset: __m256,
    min: __m256,
    max: __m256,
}

impl Splat {
    #[target_feature(enable = "avx2")]
    fn new(affine: ChannelAffine) -> Self {
        Self {
            scale: _mm256_set1_ps(affine.scale),
            offset: _mm256_set1_ps(affine.offset),
            min: _mm256_setzero_ps(),
            max: _mm256_set1_ps(affine.max),
        }
    }

    /// Eight unsigned integer channel values, widened to `i32` lanes, in and out.
    #[inline]
    #[target_feature(enable = "avx2")]
    fn apply_i32(self, values: __m256i) -> __m256i {
        let scaled = _mm256_add_ps(
            _mm256_mul_ps(_mm256_cvtepi32_ps(values), self.scale),
            self.offset,
        );
        // `cvtps_epi32` rounds to nearest, ties to even, matching the scalar
        // reference's `round_ties_even`.
        _mm256_cvtps_epi32(_mm256_min_ps(_mm256_max_ps(scaled, self.min), self.max))
    }

    /// Eight `f32` channel values in and out.
    #[inline]
    #[target_feature(enable = "avx2")]
    fn apply_f32(self, values: __m256) -> __m256 {
        let scaled = _mm256_add_ps(_mm256_mul_ps(values, self.scale), self.offset);
        _mm256_min_ps(_mm256_max_ps(scaled, self.min), self.max)
    }

    /// 32 `u8` channel values in and out.
    ///
    /// The two packs work within 128-bit halves, so the bytes come out
    /// interleaved by quarter and a final dword permute puts them back in order.
    #[inline]
    #[target_feature(enable = "avx2")]
    unsafe fn apply_u8x32(self, bytes: *const u8) -> __m256i {
        unsafe {
            let r0 = self.apply_i32(_mm256_cvtepu8_epi32(_mm_loadu_si64(bytes)));
            let r1 = self.apply_i32(_mm256_cvtepu8_epi32(_mm_loadu_si64(bytes.add(8))));
            let r2 = self.apply_i32(_mm256_cvtepu8_epi32(_mm_loadu_si64(bytes.add(16))));
            let r3 = self.apply_i32(_mm256_cvtepu8_epi32(_mm_loadu_si64(bytes.add(24))));

            let packed =
                _mm256_packus_epi16(_mm256_packs_epi32(r0, r1), _mm256_packs_epi32(r2, r3));
            // Dwords land as [r0lo r1lo r2lo r3lo r0hi r1hi r2hi r3hi].
            _mm256_permutevar8x32_epi32(packed, _mm256_setr_epi32(0, 4, 1, 5, 2, 6, 3, 7))
        }
    }

    /// 16 `u16` channel values in and out, with the same in-lane pack fixup.
    #[inline]
    #[target_feature(enable = "avx2")]
    unsafe fn apply_u16x16(self, values: *const u16) -> __m256i {
        unsafe {
            let r0 = self.apply_i32(_mm256_cvtepu16_epi32(_mm_loadu_si128(values.cast())));
            let r1 = self.apply_i32(_mm256_cvtepu16_epi32(_mm_loadu_si128(values.add(8).cast())));
            // Already clamped to `[0, 65535]`, so the saturation is a no-op.
            let packed = _mm256_packus_epi32(r0, r1);
            // Qwords land as [r0lo r1lo r0hi r1hi].
            _mm256_permute4x64_epi64(packed, 0b11_01_10_00)
        }
    }
}

/// 32 `u8` channel values per iteration.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn u8_flat(row: &mut [u8], count: usize, affine: ChannelAffine) {
    let splat = Splat::new(affine);
    let row = &mut row[..count];

    let (chunks, tail) = row.as_chunks_mut::<32>();
    for chunk in chunks {
        // SAFETY: `chunk` is exactly 32 bytes, the width of one load and store.
        unsafe {
            let out = splat.apply_u8x32(chunk.as_ptr());
            _mm256_storeu_si256(chunk.as_mut_ptr().cast(), out);
        }
    }

    for value in tail {
        *value = value.apply(affine);
    }
}

/// Eight `RGBA_U8` pixels per iteration, alpha carried through untouched.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn u8_rgba(row: &mut [u8], pixels: usize, affine: ChannelAffine) {
    let splat = Splat::new(affine);
    let row = &mut row[..pixels * 4];
    // The top byte of each dword is alpha; a set lane byte takes the original.
    let alpha_mask = _mm256_set1_epi32(0xFF00_0000_u32 as i32);

    let (chunks, tail) = row.as_chunks_mut::<32>();
    for chunk in chunks {
        // SAFETY: `chunk` is exactly 32 bytes — eight RGBA pixels.
        unsafe {
            let original = _mm256_loadu_si256(chunk.as_ptr().cast());
            let out = splat.apply_u8x32(chunk.as_ptr());
            let blended = _mm256_blendv_epi8(out, original, alpha_mask);
            _mm256_storeu_si256(chunk.as_mut_ptr().cast(), blended);
        }
    }

    for pixel in tail.chunks_exact_mut(4) {
        for value in &mut pixel[..3] {
            *value = value.apply(affine);
        }
    }
}

/// 16 `u16` channel values per iteration.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn u16_flat(row: &mut [u8], count: usize, affine: ChannelAffine) {
    let splat = Splat::new(affine);
    let row: &mut [u16] = bytemuck::cast_slice_mut(&mut row[..count * 2]);

    let (chunks, tail) = row.as_chunks_mut::<16>();
    for chunk in chunks {
        // SAFETY: `chunk` is exactly 16 `u16` — 32 bytes, one load and store.
        unsafe {
            let out = splat.apply_u16x16(chunk.as_ptr());
            _mm256_storeu_si256(chunk.as_mut_ptr().cast(), out);
        }
    }

    for value in tail {
        *value = value.apply(affine);
    }
}

/// Four `RGBA_U16` pixels per iteration, alpha carried through untouched.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn u16_rgba(row: &mut [u8], pixels: usize, affine: ChannelAffine) {
    let splat = Splat::new(affine);
    let row: &mut [u16] = bytemuck::cast_slice_mut(&mut row[..pixels * 8]);

    let (chunks, tail) = row.as_chunks_mut::<16>();
    for chunk in chunks {
        // SAFETY: `chunk` is exactly 16 `u16` — four RGBA pixels.
        unsafe {
            let original = _mm256_loadu_si256(chunk.as_ptr().cast());
            let out = splat.apply_u16x16(chunk.as_ptr());
            // The imm8 repeats per 128-bit half, so it names lanes 3, 7, 11
            // and 15 — the four alphas.
            let blended = _mm256_blend_epi16(out, original, 0b1000_1000);
            _mm256_storeu_si256(chunk.as_mut_ptr().cast(), blended);
        }
    }

    for pixel in tail.chunks_exact_mut(4) {
        for value in &mut pixel[..3] {
            *value = value.apply(affine);
        }
    }
}

/// Eight `f32` channel values per iteration.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn f32_flat(row: &mut [u8], count: usize, affine: ChannelAffine) {
    let splat = Splat::new(affine);
    let row: &mut [f32] = bytemuck::cast_slice_mut(&mut row[..count * 4]);

    let (chunks, tail) = row.as_chunks_mut::<8>();
    for chunk in chunks {
        // SAFETY: `chunk` is exactly eight `f32` — one load and store.
        unsafe {
            let values = _mm256_loadu_ps(chunk.as_ptr());
            _mm256_storeu_ps(chunk.as_mut_ptr(), splat.apply_f32(values));
        }
    }

    for value in tail {
        *value = value.apply(affine);
    }
}

/// Two `RGBA_F32` pixels per iteration, alpha carried through untouched.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn f32_rgba(row: &mut [u8], pixels: usize, affine: ChannelAffine) {
    let splat = Splat::new(affine);
    let row: &mut [f32] = bytemuck::cast_slice_mut(&mut row[..pixels * 16]);

    let (chunks, tail) = row.as_chunks_mut::<8>();
    for chunk in chunks {
        // SAFETY: `chunk` is exactly eight `f32` — two RGBA pixels.
        unsafe {
            let values = _mm256_loadu_ps(chunk.as_ptr());
            // Lanes 3 and 7 are the two alphas; take those from the original.
            let blended = _mm256_blend_ps(splat.apply_f32(values), values, 0b1000_1000);
            _mm256_storeu_ps(chunk.as_mut_ptr(), blended);
        }
    }

    for pixel in tail.chunks_exact_mut(4) {
        for value in &mut pixel[..3] {
            *value = value.apply(affine);
        }
    }
}
