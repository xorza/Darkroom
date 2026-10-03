//! Row, column and 2D convolution as vector kernels.
//!
//! The row kernel keeps each lane's sum in a register across the taps. The column and 2D kernels
//! run tap by tap instead: they zero the output row, then add `input · weight` for one tap across
//! the whole row before they take the next, so their inner loop is a stream of independent vectors
//! with no window or mirror arithmetic. Every pixel sums its taps in the scalar order with an
//! unfused multiply then add, so each kernel is bit-identical to the scalar reference: the
//! threshold that reads the filtered image is itself unfused to be exact at `px == threshold`,
//! which a fused sum here would undo. The pixels whose window crosses an edge take the scalar
//! per-pixel form.

use rayon::prelude::*;

use crate::math::size2us::Size2us;
use crate::simd::{F32_LANES, F32x8, Isa, Kernel};

/// A borrowed square 2D convolution kernel, stored row-major.
///
/// Bundles the coefficients with the side length they are laid out by, so the two
/// cannot disagree, and derives the centre offset every tap is measured from.
#[derive(Debug, Clone, Copy)]
pub(super) struct Kernel2d<'a> {
    weights: &'a [f32],
    size: usize,
}

impl<'a> Kernel2d<'a> {
    /// Panics if `weights` is not exactly `size × size` — a cold, once-per-image check,
    /// not per row or per pixel.
    pub(super) fn new(weights: &'a [f32], size: usize) -> Self {
        assert_eq!(
            weights.len(),
            size * size,
            "a {size}x{size} kernel needs {} weights",
            size * size
        );
        Self { weights, size }
    }

    /// Side length in taps.
    #[inline]
    pub(super) const fn size(self) -> usize {
        self.size
    }

    /// Offset from the kernel's first tap to its centre.
    #[inline]
    pub(super) const fn radius(self) -> usize {
        self.size / 2
    }

    #[inline]
    pub(super) const fn at(self, ky: usize, kx: usize) -> f32 {
        self.weights[ky * self.size + kx]
    }
}

/// Mirror boundary handling for convolution.
///
/// Maps an index that may be out of bounds to a valid index using reflection.
/// For index < 0: reflects at 0 (e.g., -1 -> 1, -2 -> 2)
/// For index >= len: reflects at len-1 (e.g., len -> len-2, len+1 -> len-3)
///
/// For indices far out of bounds, clamps to valid range after reflection.
#[inline]
pub(super) fn mirror_index(i: isize, len: usize) -> usize {
    debug_assert!(len > 0, "mirror_index requires len > 0");

    match usize::try_from(i) {
        Err(_) => i.unsigned_abs().min(len - 1),
        Ok(i) if i >= len => (2 * len).saturating_sub(2).saturating_sub(i).min(len - 1),
        Ok(i) => i,
    }
}

/// The radius of an odd 1D kernel: its centre tap's index.
#[inline]
fn radius_of(kernel: &[f32]) -> usize {
    debug_assert!(kernel.len() % 2 == 1, "a 1D kernel has a centre tap");
    kernel.len() / 2
}

/// One output pixel of a row convolution, mirrored at the row's ends.
#[inline]
fn convolve_pixel_scalar(input: &[f32], kernel: &[f32], x: usize) -> f32 {
    let radius = radius_of(kernel);
    let mut sum = 0.0f32;
    for (k, &kval) in kernel.iter().enumerate() {
        let sx = mirror_index(x as isize + k as isize - radius as isize, input.len());
        sum += input[sx] * kval;
    }
    sum
}

/// One output pixel of the 2D convolution at `(x, y)`, mirrored on both axes.
#[inline]
fn convolve_2d_pixel(
    input: &[f32],
    size: Size2us,
    x: usize,
    y: usize,
    kernel: Kernel2d<'_>,
) -> f32 {
    let radius = kernel.radius() as isize;
    let mut sum = 0.0f32;
    for ky in 0..kernel.size() {
        let sy = mirror_index(y as isize + ky as isize - radius, size.height);
        for kx in 0..kernel.size() {
            let sx = mirror_index(x as isize + kx as isize - radius, size.width);
            sum += input[sy * size.width + sx] * kernel.at(ky, kx);
        }
    }
    sum
}

/// Convolve one row with an odd 1D kernel (mirror edges), on the widest Isa this CPU has.
#[inline]
pub(super) fn convolve_row(input: &[f32], output: &mut [f32], kernel: &[f32]) {
    ConvolveRow {
        input,
        output,
        kernel,
    }
    .dispatch();
}

/// [`convolve_row`] as a kernel: a vector of pixels at a time where the whole window lies inside
/// the row, each lane summing its taps in a register, and the scalar per-pixel form at the
/// mirrored ends. The last vector overlaps the one before it and recomputes a few pixels to the
/// same values.
#[derive(Debug)]
struct ConvolveRow<'a> {
    input: &'a [f32],
    output: &'a mut [f32],
    kernel: &'a [f32],
}

impl Kernel for ConvolveRow<'_> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let width = self.input.len();
        debug_assert_eq!(self.output.len(), width, "one output per input sample");
        let radius = radius_of(self.kernel);
        let interior = radius..width.saturating_sub(radius).max(radius);

        let (head, rest) = self.output.split_at_mut(interior.start.min(width));
        let (inside, tail) = rest.split_at_mut(interior.len());
        for (x, out) in head.iter_mut().enumerate() {
            *out = convolve_pixel_scalar(self.input, self.kernel, x);
        }
        for (x, out) in (interior.end..).zip(tail) {
            *out = convolve_pixel_scalar(self.input, self.kernel, x);
        }
        let Some(last) = inside.len().checked_sub(F32_LANES) else {
            for (x, out) in interior.zip(inside) {
                *out = convolve_pixel_scalar(self.input, self.kernel, x);
            }
            return;
        };

        // Pixel `radius + i` reads `input[i + k]` at tap `k`, so the vector at `i` reads one
        // window of `taps + 7` samples, which the taps step through a sample at a time.
        let span = self.kernel.len() + F32_LANES - 1;
        let mut i = 0;
        loop {
            let start = i.min(last);
            let mut sum = isa.splat_f32(0.0);
            let window = &self.input[start..start + span];
            for (samples, &weight) in window.windows(F32_LANES).zip(self.kernel) {
                let samples = samples.first_chunk().expect("a window is a full vector");
                sum = sum + isa.load_f32(samples) * isa.splat_f32(weight);
            }
            sum.store(
                inside[start..]
                    .first_chunk_mut()
                    .expect("a full vector inside the interior"),
            );
            if start == last {
                break;
            }
            i += F32_LANES;
        }
    }
}

/// Column (vertical) convolution over the whole image. Each output row depends on input rows
/// `[y-radius, y+radius]` (mirror edges), so rows are independent and computed in parallel across
/// rayon workers — one row per chunk, vectors across the columns within a row.
pub(super) fn convolve_cols_direct(
    input: &[f32],
    output: &mut [f32],
    size: Size2us,
    kernel: &[f32],
) {
    assert_eq!(input.len(), size.pixel_count(), "the input fills the image");
    output
        .par_chunks_mut(size.width)
        .enumerate()
        .for_each(|(y, out_row)| {
            ConvolveColsRow {
                input,
                out_row,
                size,
                y,
                kernel,
            }
            .dispatch();
        });
}

/// One output row `y` of the column pass, tap by tap across the whole row: the column taps are
/// whole rows, mirrored at the image's top and bottom, so no pixel needs the scalar form.
#[derive(Debug)]
struct ConvolveColsRow<'a> {
    input: &'a [f32],
    out_row: &'a mut [f32],
    size: Size2us,
    y: usize,
    kernel: &'a [f32],
}

impl Kernel for ConvolveColsRow<'_> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let Self {
            input,
            out_row,
            size,
            y,
            kernel,
        } = self;
        debug_assert_eq!(out_row.len(), size.width, "one output row");
        let radius = radius_of(kernel);
        out_row.fill(0.0);
        for (k, &weight) in kernel.iter().enumerate() {
            let sy = mirror_index(y as isize + k as isize - radius as isize, size.height);
            add_weighted(
                isa,
                out_row,
                &input[sy * size.width..][..size.width],
                weight,
            );
        }
    }
}

/// Convolve one output row `y` with a 2D kernel (mirror edges on both axes), on the widest Isa
/// this CPU has.
#[inline]
pub(super) fn convolve_2d_row(
    input: &[f32],
    output_row: &mut [f32],
    size: Size2us,
    y: usize,
    kernel: Kernel2d<'_>,
) {
    Convolve2dRow {
        input,
        output_row,
        size,
        y,
        kernel,
    }
    .dispatch();
}

/// [`convolve_2d_row`] as a kernel: tap by tap over the pixels whose window lies inside the row,
/// each tap's source row mirrored at the image's top and bottom, and the scalar per-pixel form for
/// the pixels whose window crosses the row's ends.
#[derive(Debug)]
struct Convolve2dRow<'a> {
    input: &'a [f32],
    output_row: &'a mut [f32],
    size: Size2us,
    y: usize,
    kernel: Kernel2d<'a>,
}

impl Kernel for Convolve2dRow<'_> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let Self {
            input,
            output_row,
            size,
            y,
            kernel,
        } = self;
        debug_assert_eq!(output_row.len(), size.width, "one output row");
        let radius = kernel.radius();
        let interior = radius..size.width.saturating_sub(radius).max(radius);

        let (head, rest) = output_row.split_at_mut(interior.start.min(size.width));
        let (inside, tail) = rest.split_at_mut(interior.len());
        for (x, out) in head.iter_mut().enumerate() {
            *out = convolve_2d_pixel(input, size, x, y, kernel);
        }
        for (x, out) in (interior.end..).zip(tail) {
            *out = convolve_2d_pixel(input, size, x, y, kernel);
        }
        if inside.is_empty() {
            return;
        }
        inside.fill(0.0);
        for ky in 0..kernel.size() {
            let sy = mirror_index(y as isize + ky as isize - radius as isize, size.height);
            let row = &input[sy * size.width..][..size.width];
            for kx in 0..kernel.size() {
                add_weighted(
                    isa,
                    inside,
                    &row[kx..kx + interior.len()],
                    kernel.at(ky, kx),
                );
            }
        }
    }
}

/// `output[i] += input[i] · weight` over two slices of one length, unfused: one tap of a
/// convolution, in the order the scalar sum adds it.
#[inline(always)]
fn add_weighted<S: Isa>(isa: S, output: &mut [f32], input: &[f32], weight: f32) {
    debug_assert_eq!(output.len(), input.len(), "one input per output");
    let lanes = isa.splat_f32(weight);
    let (output_chunks, output_tail) = output.as_chunks_mut::<F32_LANES>();
    let (input_chunks, input_tail) = input.as_chunks::<F32_LANES>();
    for (out, samples) in output_chunks.iter_mut().zip(input_chunks) {
        (isa.load_f32(out) + isa.load_f32(samples) * lanes).store(out);
    }
    for (out, &sample) in output_tail.iter_mut().zip(input_tail) {
        *out += sample * weight;
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::math::size2us::Size2us;
    use crate::star_detection::convolution::simd::{
        Kernel2d, convolve_2d_pixel, convolve_pixel_scalar, mirror_index,
    };

    /// The scalar row convolution the kernel is tested and benched against.
    pub(crate) fn convolve_row_scalar(input: &[f32], output: &mut [f32], kernel: &[f32]) {
        for (x, out) in output.iter_mut().enumerate() {
            *out = convolve_pixel_scalar(input, kernel, x);
        }
    }

    /// The scalar column pass over one output row, mirrored at the image's top and bottom.
    pub(crate) fn convolve_cols_row_scalar(
        input: &[f32],
        out_row: &mut [f32],
        size: Size2us,
        y: usize,
        kernel: &[f32],
    ) {
        let radius = kernel.len() / 2;
        for (x, out) in out_row.iter_mut().enumerate() {
            let mut sum = 0.0f32;
            for (k, &kval) in kernel.iter().enumerate() {
                let sy = mirror_index(y as isize + k as isize - radius as isize, size.height);
                sum += input[sy * size.width + x] * kval;
            }
            *out = sum;
        }
    }

    /// The scalar 2D convolution of one output row.
    pub(crate) fn convolve_2d_row_scalar(
        input: &[f32],
        output_row: &mut [f32],
        size: Size2us,
        y: usize,
        kernel: Kernel2d<'_>,
    ) {
        for (x, out) in output_row.iter_mut().enumerate() {
            *out = convolve_2d_pixel(input, size, x, y, kernel);
        }
    }
}

#[cfg(test)]
mod tests;
