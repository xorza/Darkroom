//! The u16 → normalized f32 conversion as a vector kernel.

use crate::io::raw::normalize::normalize_one;
use crate::simd::{F32_LANES, F32x8, Isa, Kernel};

/// Normalize one chunk on the widest Isa this CPU has.
#[inline]
pub(super) fn normalize_chunk<const CLAMP: bool>(
    input: &[u16],
    output: &mut [f32],
    black: f32,
    span: f32,
) {
    NormalizeChunk::<CLAMP> {
        input,
        output,
        black,
        span,
    }
    .dispatch();
}

/// [`normalize_one`] over `input` into `output`, [`F32_LANES`] samples at a time. The vector
/// steps are `normalize_one`'s own IEEE operations in its order, so the scalar tail matches them
/// bit for bit.
#[derive(Debug)]
struct NormalizeChunk<'a, const CLAMP: bool> {
    input: &'a [u16],
    output: &'a mut [f32],
    black: f32,
    span: f32,
}

impl<const CLAMP: bool> Kernel for NormalizeChunk<'_, CLAMP> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let black = isa.splat_f32(self.black);
        let span = isa.splat_f32(self.span);
        let (input_chunks, input_tail) = self.input.as_chunks::<F32_LANES>();
        let (output_chunks, output_tail) = self.output.as_chunks_mut::<F32_LANES>();

        for (input, output) in input_chunks.iter().zip(output_chunks) {
            let subtracted = isa.load_u16(input) - black;
            let normalized = if CLAMP {
                (subtracted.max(isa.splat_f32(0.0)) / span).min(isa.splat_f32(1.0))
            } else {
                subtracted / span
            };
            normalized.store(output);
        }
        for (&value, output) in input_tail.iter().zip(output_tail) {
            *output = normalize_one::<CLAMP>(value, self.black, self.span);
        }
    }
}

#[cfg(test)]
mod tests;
