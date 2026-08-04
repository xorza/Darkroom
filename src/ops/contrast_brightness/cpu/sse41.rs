//! SSE4.1 row kernels. Each applies [`ChannelAffine`] to a row in place and
//! finishes the sub-vector tail with the scalar reference, so the vector body
//! and the tail produce identical results.

use std::arch::x86_64::*;

use crate::ops::contrast_brightness::cpu::{ChannelAffine, ContrastBrightnessApply};

/// The affine's constants, splatted once per row.
#[derive(Debug, Clone, Copy)]
struct Splat {
    scale: __m128,
    offset: __m128,
    min: __m128,
    max: __m128,
}

impl Splat {
    #[target_feature(enable = "sse4.1")]
    fn new(affine: ChannelAffine) -> Self {
        Self {
            scale: _mm_set1_ps(affine.scale),
            offset: _mm_set1_ps(affine.offset),
            min: _mm_setzero_ps(),
            max: _mm_set1_ps(affine.max),
        }
    }

    /// Four unsigned integer channel values, widened to `i32` lanes, in and out.
    #[inline]
    #[target_feature(enable = "sse4.1")]
    fn apply_i32(self, values: __m128i) -> __m128i {
        let scaled = _mm_add_ps(_mm_mul_ps(_mm_cvtepi32_ps(values), self.scale), self.offset);
        // `cvtps_epi32` rounds to nearest, ties to even, matching the scalar
        // reference's `round_ties_even`.
        _mm_cvtps_epi32(_mm_min_ps(_mm_max_ps(scaled, self.min), self.max))
    }

    /// Four `f32` channel values in and out.
    #[inline]
    #[target_feature(enable = "sse4.1")]
    fn apply_f32(self, values: __m128) -> __m128 {
        let scaled = _mm_add_ps(_mm_mul_ps(values, self.scale), self.offset);
        _mm_min_ps(_mm_max_ps(scaled, self.min), self.max)
    }
}

/// 16 `u8` channel values per iteration.
#[target_feature(enable = "sse4.1")]
pub(super) unsafe fn u8_flat(row: &mut [u8], count: usize, affine: ChannelAffine) {
    let splat = Splat::new(affine);
    let row = &mut row[..count];
    let zero = _mm_setzero_si128();

    let (chunks, tail) = row.as_chunks_mut::<16>();
    for chunk in chunks {
        // SAFETY: `chunk` is exactly 16 bytes, the width of one load and store.
        unsafe {
            let bytes = _mm_loadu_si128(chunk.as_ptr().cast());
            let lo = _mm_unpacklo_epi8(bytes, zero);
            let hi = _mm_unpackhi_epi8(bytes, zero);

            let r0 = splat.apply_i32(_mm_unpacklo_epi16(lo, zero));
            let r1 = splat.apply_i32(_mm_unpackhi_epi16(lo, zero));
            let r2 = splat.apply_i32(_mm_unpacklo_epi16(hi, zero));
            let r3 = splat.apply_i32(_mm_unpackhi_epi16(hi, zero));

            let packed = _mm_packus_epi16(_mm_packs_epi32(r0, r1), _mm_packs_epi32(r2, r3));
            _mm_storeu_si128(chunk.as_mut_ptr().cast(), packed);
        }
    }

    for value in tail {
        *value = value.apply(affine);
    }
}

/// Four `RGBA_U8` pixels per iteration, alpha carried through untouched.
#[target_feature(enable = "sse4.1")]
pub(super) unsafe fn u8_rgba(row: &mut [u8], pixels: usize, affine: ChannelAffine) {
    let splat = Splat::new(affine);
    let row = &mut row[..pixels * 4];
    let zero = _mm_setzero_si128();
    // Every fourth byte is alpha; a set lane byte takes the original.
    let alpha_mask = _mm_setr_epi8(0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1);

    let (chunks, tail) = row.as_chunks_mut::<16>();
    for chunk in chunks {
        // SAFETY: `chunk` is exactly 16 bytes — four RGBA pixels.
        unsafe {
            let bytes = _mm_loadu_si128(chunk.as_ptr().cast());
            let lo = _mm_unpacklo_epi8(bytes, zero);
            let hi = _mm_unpackhi_epi8(bytes, zero);

            let r0 = splat.apply_i32(_mm_unpacklo_epi16(lo, zero));
            let r1 = splat.apply_i32(_mm_unpackhi_epi16(lo, zero));
            let r2 = splat.apply_i32(_mm_unpacklo_epi16(hi, zero));
            let r3 = splat.apply_i32(_mm_unpackhi_epi16(hi, zero));

            let packed = _mm_packus_epi16(_mm_packs_epi32(r0, r1), _mm_packs_epi32(r2, r3));
            let blended = _mm_blendv_epi8(packed, bytes, alpha_mask);
            _mm_storeu_si128(chunk.as_mut_ptr().cast(), blended);
        }
    }

    for pixel in tail.chunks_exact_mut(4) {
        for value in &mut pixel[..3] {
            *value = value.apply(affine);
        }
    }
}

/// Eight `u16` channel values per iteration.
#[target_feature(enable = "sse4.1")]
pub(super) unsafe fn u16_flat(row: &mut [u8], count: usize, affine: ChannelAffine) {
    let splat = Splat::new(affine);
    let row: &mut [u16] = bytemuck::cast_slice_mut(&mut row[..count * 2]);
    let zero = _mm_setzero_si128();

    let (chunks, tail) = row.as_chunks_mut::<8>();
    for chunk in chunks {
        // SAFETY: `chunk` is exactly eight `u16` — 16 bytes, one load and store.
        unsafe {
            let values = _mm_loadu_si128(chunk.as_ptr().cast());
            let lo = splat.apply_i32(_mm_unpacklo_epi16(values, zero));
            let hi = splat.apply_i32(_mm_unpackhi_epi16(values, zero));
            // Already clamped to `[0, 65535]`, so the saturation is a no-op.
            _mm_storeu_si128(chunk.as_mut_ptr().cast(), _mm_packus_epi32(lo, hi));
        }
    }

    for value in tail {
        *value = value.apply(affine);
    }
}

/// Two `RGBA_U16` pixels per iteration, alpha carried through untouched.
#[target_feature(enable = "sse4.1")]
pub(super) unsafe fn u16_rgba(row: &mut [u8], pixels: usize, affine: ChannelAffine) {
    let splat = Splat::new(affine);
    let row: &mut [u16] = bytemuck::cast_slice_mut(&mut row[..pixels * 8]);
    let zero = _mm_setzero_si128();

    let (chunks, tail) = row.as_chunks_mut::<8>();
    for chunk in chunks {
        // SAFETY: `chunk` is exactly eight `u16` — two RGBA pixels.
        unsafe {
            let values = _mm_loadu_si128(chunk.as_ptr().cast());
            let lo = splat.apply_i32(_mm_unpacklo_epi16(values, zero));
            let hi = splat.apply_i32(_mm_unpackhi_epi16(values, zero));
            let packed = _mm_packus_epi32(lo, hi);
            // Lanes 3 and 7 are the two alphas; take those from the original.
            let blended = _mm_blend_epi16(packed, values, 0b1000_1000);
            _mm_storeu_si128(chunk.as_mut_ptr().cast(), blended);
        }
    }

    for pixel in tail.chunks_exact_mut(4) {
        for value in &mut pixel[..3] {
            *value = value.apply(affine);
        }
    }
}

/// Four `f32` channel values per iteration.
#[target_feature(enable = "sse4.1")]
pub(super) unsafe fn f32_flat(row: &mut [u8], count: usize, affine: ChannelAffine) {
    let splat = Splat::new(affine);
    let row: &mut [f32] = bytemuck::cast_slice_mut(&mut row[..count * 4]);

    let (chunks, tail) = row.as_chunks_mut::<4>();
    for chunk in chunks {
        // SAFETY: `chunk` is exactly four `f32` — one load and store.
        unsafe {
            let values = _mm_loadu_ps(chunk.as_ptr());
            _mm_storeu_ps(chunk.as_mut_ptr(), splat.apply_f32(values));
        }
    }

    for value in tail {
        *value = value.apply(affine);
    }
}

/// One `RGBA_F32` pixel per iteration — a pixel already fills the vector —
/// with alpha carried through untouched.
#[target_feature(enable = "sse4.1")]
pub(super) unsafe fn f32_rgba(row: &mut [u8], pixels: usize, affine: ChannelAffine) {
    let splat = Splat::new(affine);
    let row: &mut [f32] = bytemuck::cast_slice_mut(&mut row[..pixels * 16]);

    let (chunks, tail) = row.as_chunks_mut::<4>();
    for chunk in chunks {
        // SAFETY: `chunk` is exactly four `f32` — one RGBA pixel.
        unsafe {
            let values = _mm_loadu_ps(chunk.as_ptr());
            // Lane 3 is alpha; take it from the original.
            let blended = _mm_blend_ps(splat.apply_f32(values), values, 0b1000);
            _mm_storeu_ps(chunk.as_mut_ptr(), blended);
        }
    }

    // A pixel fills the vector exactly, so this is empty — kept so the kernel
    // stays correct if the chunk ever covers more than one pixel.
    for pixel in tail.chunks_exact_mut(4) {
        for value in &mut pixel[..3] {
            *value = value.apply(affine);
        }
    }
}
