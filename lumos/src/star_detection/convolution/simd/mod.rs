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

use common::{Size2us, Vec2us};
use simd::{F32_LANES, F32x8, Isa, Kernel};

/// The rows a column or 2D convolution reads its taps from: whole rows of an image of `size`, image
/// row `n` in slot `n` modulo the slots held. A ring of a filter's tap count holds every row a band
/// of output reads; a whole image is the ring of all its rows.
#[derive(Debug, Clone, Copy)]
pub(super) struct RowRing<'a> {
    rows: &'a [f32],
    size: Size2us,
}

impl<'a> RowRing<'a> {
    /// `rows`, whole rows of an image of `size`.
    pub(super) fn new(rows: &'a [f32], size: Size2us) -> Self {
        debug_assert!(
            !rows.is_empty() && rows.len().is_multiple_of(size.width),
            "a ring holds whole rows"
        );
        Self { rows, size }
    }

    /// Image row `n`.
    #[inline]
    fn row(self, n: usize) -> &'a [f32] {
        let width = self.size.width;
        let slot = n % (self.rows.len() / width);
        &self.rows[slot * width..(slot + 1) * width]
    }
}

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
    /// `weights` is exactly `size × size`.
    pub(super) fn new(weights: &'a [f32], size: usize) -> Self {
        debug_assert_eq!(
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

    /// One output pixel of the 2D convolution of the image `input` at `pos`, mirrored on both axes.
    #[inline]
    fn convolve_pixel(self, input: RowRing<'_>, pos: Vec2us) -> f32 {
        let size = input.size;
        let radius = self.radius() as isize;
        let mut sum = 0.0f32;
        for ky in 0..self.size() {
            let row = input.row(mirror_idx(
                pos.y as isize + ky as isize - radius,
                size.height,
            ));
            for kx in 0..self.size() {
                let sx = mirror_idx(pos.x as isize + kx as isize - radius, size.width);
                sum += row[sx] * self.at(ky, kx);
            }
        }
        sum
    }
}

/// Mirror boundary handling for convolution: the index `i` reflects at both ends without repeating
/// the edge sample (`−1 → 1`, `len → len − 2`), as scipy's `mirror` mode does, and again at every
/// further edge, with period `2(len − 1)`. So a kernel wider than the image is mirrored too, not
/// clamped to an edge sample. A length of 1 maps every index to 0.
#[inline]
pub(super) fn mirror_idx(i: isize, len: usize) -> usize {
    debug_assert!(len > 0, "mirror_index requires len > 0");
    if let Ok(idx) = usize::try_from(i)
        && idx < len
    {
        return idx;
    }
    let period = 2 * (len - 1);
    if period == 0 {
        return 0;
    }
    let phase = i.rem_euclid(period.cast_signed()).cast_unsigned();
    if phase < len { phase } else { period - phase }
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
        let sx = mirror_idx(x as isize + k as isize - radius as isize, input.len());
        sum += input[sx] * kval;
    }
    sum
}

/// One row convolved with an odd 1D kernel (mirror edges): a vector of pixels at a time where the
/// whole window lies inside the row, each lane summing its taps in a register, and the scalar
/// per-pixel form at the mirrored ends. The last vector overlaps the one before it and recomputes a
/// few pixels to the same values.
#[derive(Debug)]
pub(super) struct ConvolveRow<'a> {
    pub(super) input: &'a [f32],
    pub(super) output: &'a mut [f32],
    pub(super) kernel: &'a [f32],
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

/// One output row `y` of the column pass over the image `input`, tap by tap across the
/// whole row: the column taps are whole rows, mirrored at the image's top and bottom, so no pixel
/// needs the scalar form.
#[derive(Debug)]
pub(super) struct ConvolveColsRow<'a> {
    pub(super) input: RowRing<'a>,
    pub(super) out_row: &'a mut [f32],
    pub(super) y: usize,
    pub(super) kernel: &'a [f32],
}

impl Kernel for ConvolveColsRow<'_> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let Self {
            input,
            out_row,
            y,
            kernel,
        } = self;
        let size = input.size;
        debug_assert_eq!(out_row.len(), size.width, "one output row");
        let radius = radius_of(kernel);
        out_row.fill(0.0);
        for (k, &weight) in kernel.iter().enumerate() {
            let sy = mirror_idx(y as isize + k as isize - radius as isize, size.height);
            WeightedTap {
                input: input.row(sy),
                weight,
            }
            .add_to(isa, out_row);
        }
    }
}

/// One output row `y` of the 2D convolution of the image `input` (mirror edges on both
/// axes): tap by tap over the pixels whose window lies inside the row, each tap's source row
/// mirrored at the image's top and bottom, and the scalar per-pixel form for the pixels whose
/// window crosses the row's ends.
#[derive(Debug)]
pub(super) struct Convolve2dRow<'a> {
    pub(super) input: RowRing<'a>,
    pub(super) output_row: &'a mut [f32],
    pub(super) y: usize,
    pub(super) kernel: Kernel2d<'a>,
}

impl Kernel for Convolve2dRow<'_> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let Self {
            input,
            output_row,
            y,
            kernel,
        } = self;
        let size = input.size;
        debug_assert_eq!(output_row.len(), size.width, "one output row");
        let radius = kernel.radius();
        let interior = radius..size.width.saturating_sub(radius).max(radius);

        let (head, rest) = output_row.split_at_mut(interior.start.min(size.width));
        let (inside, tail) = rest.split_at_mut(interior.len());
        for (x, out) in head.iter_mut().enumerate() {
            *out = kernel.convolve_pixel(input, Vec2us::new(x, y));
        }
        for (x, out) in (interior.end..).zip(tail) {
            *out = kernel.convolve_pixel(input, Vec2us::new(x, y));
        }
        if inside.is_empty() {
            return;
        }
        inside.fill(0.0);
        for ky in 0..kernel.size() {
            let sy = mirror_idx(y as isize + ky as isize - radius as isize, size.height);
            let row = input.row(sy);
            for kx in 0..kernel.size() {
                WeightedTap {
                    input: &row[kx..kx + interior.len()],
                    weight: kernel.at(ky, kx),
                }
                .add_to(isa, inside);
            }
        }
    }
}

/// One tap of a convolution over a row: the input samples it reads, and their weight.
#[derive(Debug, Clone, Copy)]
struct WeightedTap<'a> {
    input: &'a [f32],
    weight: f32,
}

impl WeightedTap<'_> {
    /// `output[i] += input[i] · weight` over two slices of one length, unfused, in the order the
    /// scalar sum adds it.
    #[inline(always)]
    fn add_to<S: Isa>(self, isa: S, output: &mut [f32]) {
        let Self { input, weight } = self;
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
}

#[cfg(feature = "internals")]
mod internals {
    #[cfg(test)]
    use crate::star_detection::convolution::simd::RowRing;
    #[cfg(test)]
    use common::Buffer2;

    #[cfg(test)]
    impl<'a> RowRing<'a> {
        /// Every row of `image`, each in its own slot.
        pub(crate) fn plane(image: &'a Buffer2<f32>) -> Self {
            Self::new(image.pixels(), image.size())
        }
    }

    #[cfg(test)]
    use crate::star_detection::convolution::simd::{Convolve2dRow, ConvolveColsRow, mirror_idx};
    use crate::star_detection::convolution::simd::{ConvolveRow, convolve_pixel_scalar};
    #[cfg(test)]
    use common::Vec2us;

    impl ConvolveRow<'_> {
        /// The scalar row convolution the kernel is tested and benched against.
        pub(super) fn run_scalar(self) {
            for (x, out) in self.output.iter_mut().enumerate() {
                *out = convolve_pixel_scalar(self.input, self.kernel, x);
            }
        }
    }

    #[cfg(test)]
    impl ConvolveColsRow<'_> {
        /// The scalar column pass over one output row, mirrored at the image's top and bottom.
        pub(super) fn run_scalar(self) {
            let radius = self.kernel.len() / 2;
            for (x, out) in self.out_row.iter_mut().enumerate() {
                let mut sum = 0.0f32;
                for (k, &kval) in self.kernel.iter().enumerate() {
                    let sy = mirror_idx(
                        self.y as isize + k as isize - radius as isize,
                        self.input.size.height,
                    );
                    sum += self.input.row(sy)[x] * kval;
                }
                *out = sum;
            }
        }
    }

    #[cfg(test)]
    impl Convolve2dRow<'_> {
        /// The scalar 2D convolution of one output row.
        pub(super) fn run_scalar(self) {
            for (x, out) in self.output_row.iter_mut().enumerate() {
                *out = self
                    .kernel
                    .convolve_pixel(self.input, Vec2us::new(x, self.y));
            }
        }
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
