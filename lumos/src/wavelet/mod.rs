//! Isotropic undecimated ("à trous" / **starlet**) wavelet transform: a B3-spline separable
//! convolution with `2^j` hole spacing per scale. Redundant, shift-invariant, flux-conserving, and
//! exact-reconstructing — `image == residual + Σ layers`.
//!
//! Shared multiscale primitive: `denoise` thresholds the detail layers, `hdr` compresses the coarse
//! component, and multiscale sharpening (future) amplifies chosen layers. `denoise` streams the
//! transform (never storing all layers) via [`atrous_smooth`]; `hdr` needs the layers explicitly via
//! [`StarletTransform`].

use common::Buffer2;
use rayon::prelude::*;

#[cfg(test)]
mod tests;

/// B3-spline low-pass filter `[1, 4, 6, 4, 1] / 16` — the separable à trous smoothing kernel.
const B3: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

/// A starlet decomposition of a plane: `image == residual + Σ layers`. `layers[0]` is the finest
/// detail (~1 px structures), `layers[j]` covers ~`2^j` px; `residual` is the coarsest smooth.
#[derive(Debug, Clone)]
pub(crate) struct StarletTransform {
    pub(crate) layers: Vec<Buffer2<f32>>,
    pub(crate) residual: Buffer2<f32>,
}

impl StarletTransform {
    /// Forward à trous transform into `scales` detail layers + a residual.
    pub(crate) fn forward(image: &Buffer2<f32>, scales: usize) -> Self {
        let (w, h) = (image.width(), image.height());
        let mut c_curr = image.clone();
        let mut c_next = Buffer2::new_default(w, h);
        let mut tmp = Buffer2::new_default(w, h);
        let mut layers = Vec::with_capacity(scales);
        for j in 0..scales {
            atrous_smooth(&c_curr, &mut c_next, &mut tmp, 1 << j); // c_{j+1}
            // Detail layer w_{j+1} = c_j − c_{j+1}.
            let mut detail = Buffer2::new_default(w, h);
            detail
                .pixels_mut()
                .par_iter_mut()
                .zip(c_curr.pixels().par_iter())
                .zip(c_next.pixels().par_iter())
                .for_each(|((d, &cc), &cn)| *d = cc - cn);
            layers.push(detail);
            std::mem::swap(&mut c_curr, &mut c_next);
        }
        Self {
            layers,
            residual: c_curr,
        }
    }

    /// Reconstruct `residual + Σ layers` into a single plane (exact up to f32 rounding).
    pub(crate) fn reconstruct(&self) -> Buffer2<f32> {
        let mut out = self.residual.clone();
        for layer in &self.layers {
            out.pixels_mut()
                .par_iter_mut()
                .zip(layer.pixels().par_iter())
                .for_each(|(o, &d)| *o += d);
        }
        out
    }
}

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
    dst.pixels_mut()
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, out)| {
            let row = src.row(y);
            for x in 0..width {
                out[x] = if x >= 2 * step && x + 2 * step < width {
                    B3[0] * row[x - 2 * step]
                        + B3[1] * row[x - step]
                        + B3[2] * row[x]
                        + B3[3] * row[x + step]
                        + B3[4] * row[x + 2 * step]
                } else {
                    let xi = x as isize;
                    B3[0] * row[reflect(xi - 2 * s, wi)]
                        + B3[1] * row[reflect(xi - s, wi)]
                        + B3[2] * row[x]
                        + B3[3] * row[reflect(xi + s, wi)]
                        + B3[4] * row[reflect(xi + 2 * s, wi)]
                };
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
            for x in 0..width {
                out[x] =
                    B3[0] * r0[x] + B3[1] * r1[x] + B3[2] * r2[x] + B3[3] * r3[x] + B3[4] * r4[x];
            }
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
