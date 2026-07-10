//! Isotropic undecimated ("à trous" / **starlet**) wavelet transform: a B3-spline separable
//! convolution with `2^j` hole spacing per scale. Redundant, shift-invariant, flux-conserving, and
//! exact-reconstructing — `image == residual + Σ layers`, where the detail `w_{j+1} = c_j − c_{j+1}`
//! telescopes out of successive smooths.
//!
//! Shared multiscale primitive: both `denoise` (thresholds the details) and `hdr` (compresses the
//! coarse residual) **stream** [`atrous_smooth`] over a rolling pair of planes and exploit the
//! telescoping identity, so the layer pyramid is never materialized.

use imaginarium::Buffer2;
use rayon::prelude::*;

#[cfg(test)]
mod tests;

/// B3-spline low-pass filter `[1, 4, 6, 4, 1] / 16` — the separable à trous smoothing kernel.
const B3: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

/// One starlet smoothing step: separable B3-spline à trous convolution with hole spacing `step`,
/// `src → dst` using `tmp` as the horizontal-pass intermediate.
pub(crate) fn atrous_smooth(
    src: &Buffer2<f32>,
    dst: &mut Buffer2<f32>,
    tmp: &mut Buffer2<f32>,
    step: usize,
) {
    convolve_horizontal(src, tmp, step);
    convolve_vertical(tmp, dst, step);
}

/// Horizontal B3-spline convolution with taps at `x ± step` and `x ± 2·step` (mirror boundary).
fn convolve_horizontal(src: &Buffer2<f32>, dst: &mut Buffer2<f32>, step: usize) {
    let width = src.width();
    let wi = width as isize;
    let s = step as isize;
    // Interior `[lo, hi)` has all five taps in-bounds; only the thin borders need reflection.
    let lo = (2 * step).min(width);
    let hi = width.saturating_sub(2 * step);
    dst.pixels_mut()
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, out)| {
            let row = src.row(y);
            let reflected = |x: usize| {
                let xi = x as isize;
                B3[0] * row[reflect(xi - 2 * s, wi)]
                    + B3[1] * row[reflect(xi - s, wi)]
                    + B3[2] * row[x]
                    + B3[3] * row[reflect(xi + s, wi)]
                    + B3[4] * row[reflect(xi + 2 * s, wi)]
            };
            for (x, o) in out[..lo].iter_mut().enumerate() {
                *o = reflected(x);
            }
            // Branchless fixed-offset stencil: zip-iterated so the per-element bounds checks elide
            // and the compiler can auto-vectorize the (vast-majority) interior.
            if hi > lo {
                let n = hi - lo;
                out[lo..hi]
                    .iter_mut()
                    .zip(row[lo - 2 * step..lo - 2 * step + n].iter())
                    .zip(row[lo - step..lo - step + n].iter())
                    .zip(row[lo..lo + n].iter())
                    .zip(row[lo + step..lo + step + n].iter())
                    .zip(row[lo + 2 * step..lo + 2 * step + n].iter())
                    .for_each(|(((((o, &m2), &m1), &c), &p1), &p2)| {
                        *o = B3[0] * m2 + B3[1] * m1 + B3[2] * c + B3[3] * p1 + B3[4] * p2;
                    });
            }
            let rstart = hi.max(lo);
            for (i, o) in out[rstart..].iter_mut().enumerate() {
                *o = reflected(rstart + i);
            }
        });
}

/// Vertical B3-spline convolution with taps at `y ± step` and `y ± 2·step` (mirror boundary).
fn convolve_vertical(src: &Buffer2<f32>, dst: &mut Buffer2<f32>, step: usize) {
    let width = src.width();
    let height = src.height();
    let hi = height as isize;
    let s = step as isize;
    dst.pixels_mut()
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, out)| {
            let (r0, r1, r2, r3, r4) = if y >= 2 * step && y + 2 * step < height {
                (
                    src.row(y - 2 * step),
                    src.row(y - step),
                    src.row(y),
                    src.row(y + step),
                    src.row(y + 2 * step),
                )
            } else {
                let yi = y as isize;
                (
                    src.row(reflect(yi - 2 * s, hi)),
                    src.row(reflect(yi - s, hi)),
                    src.row(y),
                    src.row(reflect(yi + s, hi)),
                    src.row(reflect(yi + 2 * s, hi)),
                )
            };
            // Zip-iterated (bounds checks elide) so the compiler auto-vectorizes the 5-tap column.
            out.iter_mut()
                .zip(r0.iter())
                .zip(r1.iter())
                .zip(r2.iter())
                .zip(r3.iter())
                .zip(r4.iter())
                .for_each(|(((((o, &a), &b), &c), &d), &e)| {
                    *o = B3[0] * a + B3[1] * b + B3[2] * c + B3[3] * d + B3[4] * e;
                });
        });
}

/// Mirror-reflect an index into `[0, n)` (whole-sample symmetric, no edge repeat), folding
/// arbitrary out-of-range values so even the coarsest hole step is safe on small images.
#[inline]
fn reflect(i: isize, n: isize) -> usize {
    debug_assert!(n >= 1);
    if n == 1 {
        return 0;
    }
    let period = 2 * (n - 1);
    let mut m = i % period;
    if m < 0 {
        m += period;
    }
    if m >= n {
        m = period - m;
    }
    m as usize
}

/// Largest scale count for which the coarsest hole step stays within the image: `2^J ≤ min(w, h)`.
/// Beyond it the à trous kernel spans the whole frame, so the extra scales do nothing.
pub(crate) fn max_scales(width: usize, height: usize) -> usize {
    let min_dim = width.min(height);
    if min_dim < 2 {
        return 1;
    }
    min_dim.ilog2() as usize
}
