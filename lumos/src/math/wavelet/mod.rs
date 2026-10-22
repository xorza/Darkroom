//! Isotropic undecimated ("à trous" / **starlet**) wavelet transform: a B3-spline separable
//! convolution with `2^j` hole spacing per scale. Redundant, shift-invariant, flux-conserving, and
//! exact-reconstructing — `image == residual + Σ layers`, where the detail `w_{j+1} = c_j −
//! c_{j+1}` telescopes out of successive smooths.
//!
//! Shared multiscale primitive: `denoise` (thresholds the details) and `hdr` (compresses the
//! coarse residual) **stream** [`atrous_smooth`] over a rolling pair of planes and exploit the
//! telescoping identity, so the layer pyramid is never materialized.

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::math::size2us::Size2us;
use crate::simd::{F32_LANES, F32x8, Isa, Kernel};

/// B3-spline low-pass filter `[1, 4, 6, 4, 1] / 16` — the separable à trous smoothing kernel.
const B3: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

/// One starlet smoothing step: separable B3-spline à trous convolution with hole spacing `step`,
/// `src → dst`.
///
/// Each output row is its five source rows convolved down the columns into a row of scratch, then
/// convolved along that row. Consecutive rows share their source rows, which so come from cache:
/// the frame is read once and written once, with no plane between the passes.
pub(crate) fn atrous_smooth(src: &Buffer2<f32>, dst: &mut Buffer2<f32>, step: usize) {
    let (width, height) = (src.width(), src.height());
    debug_assert_eq!((dst.width(), dst.height()), (width, height));
    let rows = height as isize;
    let s = step as isize;
    dst.pixels_mut()
        .par_chunks_mut(width)
        .enumerate()
        .for_each_init(
            || vec![0.0f32; width],
            |col, (y, out)| {
                let taps = if y >= 2 * step && y + 2 * step < height {
                    [y - 2 * step, y - step, y, y + step, y + 2 * step]
                } else {
                    let yi = y as isize;
                    [
                        reflect(yi - 2 * s, rows),
                        reflect(yi - s, rows),
                        y,
                        reflect(yi + s, rows),
                        reflect(yi + 2 * s, rows),
                    ]
                };
                SmoothRow {
                    rows: taps.map(|row| src.row(row)),
                    col,
                    out,
                    step,
                }
                .dispatch();
            },
        );
}

/// One output row of [`atrous_smooth`]: its five source `rows` convolved down the columns into
/// `col`, then `col` along itself at hole spacing `step` into `out`.
#[derive(Debug)]
struct SmoothRow<'a> {
    rows: [&'a [f32]; 5],
    col: &'a mut [f32],
    out: &'a mut [f32],
    step: usize,
}

impl Kernel for SmoothRow<'_> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let Self {
            rows,
            col,
            out,
            step,
        } = self;
        five_taps(isa, rows, col);
        let row: &[f32] = col;
        let width = row.len();
        // The interior `[lo, hi)` has all five taps in bounds; only the thin borders reflect.
        let lo = (2 * step).min(width);
        let hi = width.saturating_sub(2 * step);
        for (x, o) in out[..lo].iter_mut().enumerate() {
            *o = reflected_taps(row, x, step);
        }
        if hi > lo {
            let n = hi - lo;
            five_taps(
                isa,
                [
                    &row[lo - 2 * step..lo - 2 * step + n],
                    &row[lo - step..lo - step + n],
                    &row[lo..lo + n],
                    &row[lo + step..lo + step + n],
                    &row[lo + 2 * step..lo + 2 * step + n],
                ],
                &mut out[lo..hi],
            );
        }
        let rest = hi.max(lo);
        for (x, o) in out.iter_mut().enumerate().skip(rest) {
            *o = reflected_taps(row, x, step);
        }
    }
}

/// The B3 taps of five sample runs as long as `out`, lane by lane, into `out`:
/// `w₀a + w₁b + w₂c + w₃d + w₄e`, each product rounded, then added left to right — one order in
/// every lane of every Isa and in the scalar tail, so the transform is the same bits everywhere.
#[inline(always)]
fn five_taps<S: Isa>(isa: S, taps: [&[f32]; 5], out: &mut [f32]) {
    debug_assert!(taps.iter().all(|tap| tap.len() == out.len()));
    let weights = Weights {
        w0: isa.splat_f32(B3[0]),
        w1: isa.splat_f32(B3[1]),
        w2: isa.splat_f32(B3[2]),
        w3: isa.splat_f32(B3[3]),
        w4: isa.splat_f32(B3[4]),
    };
    // Four vectors a step, independent of one another, for the add chain of each to overlap the
    // others'; then a vector at a time, then the scalar tail.
    let (blocks, _) = out.as_chunks_mut::<{ 4 * F32_LANES }>();
    let block_len = blocks.len() * 4 * F32_LANES;
    for (idx, block) in blocks.iter_mut().enumerate() {
        let start = idx * 4 * F32_LANES;
        let (vectors, _) = block.as_chunks_mut::<F32_LANES>();
        for (lane, o) in vectors.iter_mut().enumerate() {
            weights.taps(isa, taps, start + lane * F32_LANES).store(o);
        }
    }
    let rest = &mut out[block_len..];
    let rest_len = rest.len();
    let (vectors, tail) = rest.as_chunks_mut::<F32_LANES>();
    for (idx, o) in vectors.iter_mut().enumerate() {
        weights
            .taps(isa, taps, block_len + idx * F32_LANES)
            .store(o);
    }
    let done = block_len + rest_len - tail.len();
    let [a, b, c, d, e] = taps;
    for (offset, o) in tail.iter_mut().enumerate() {
        let x = done + offset;
        *o = B3[0] * a[x] + B3[1] * b[x] + B3[2] * c[x] + B3[3] * d[x] + B3[4] * e[x];
    }
}

/// The five B3 weights, each in every lane.
#[derive(Debug, Clone, Copy)]
struct Weights<V> {
    w0: V,
    w1: V,
    w2: V,
    w3: V,
    w4: V,
}

impl<V: F32x8> Weights<V> {
    /// The taps of the vector of each run starting at `at`, in [`five_taps`]'s order.
    #[inline(always)]
    fn taps<S: Isa<F32 = V>>(self, isa: S, taps: [&[f32]; 5], at: usize) -> V {
        let [a, b, c, d, e] = taps;
        self.w0 * isa.load_f32_at(a, at)
            + self.w1 * isa.load_f32_at(b, at)
            + self.w2 * isa.load_f32_at(c, at)
            + self.w3 * isa.load_f32_at(d, at)
            + self.w4 * isa.load_f32_at(e, at)
    }
}

/// The B3 taps of `row` at `x ± step` and `x ± 2·step`, mirrored at the edges, in
/// [`five_taps`]'s order.
#[inline(always)]
fn reflected_taps(row: &[f32], x: usize, step: usize) -> f32 {
    let (xi, s, wi) = (x as isize, step as isize, row.len() as isize);
    B3[0] * row[reflect(xi - 2 * s, wi)]
        + B3[1] * row[reflect(xi - s, wi)]
        + B3[2] * row[x]
        + B3[3] * row[reflect(xi + s, wi)]
        + B3[4] * row[reflect(xi + 2 * s, wi)]
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
    let m = i.rem_euclid(period);
    if m >= n { period - m } else { m }.unsigned_abs()
}

/// The σ of detail layer `scale` for unit white noise: `√Σw²` of the layer's response to an impulse.
///
/// The smooth at scale `j` is separable, `g_j(x)·g_j(y)` with `g_j` the B3 kernel convolved with its
/// dilations up to `2^(j−1)`, so the layer `w_j = c_j − c_{j+1}` has
/// `Σw_j² = (Σg_j²)² − 2(Σg_j·g_{j+1})² + (Σg_{j+1}²)²`, exactly, at any scale. Starck & Murtagh
/// publish these as 0.889, 0.200, 0.086, 0.041 and 0.020.
pub(crate) fn white_noise_sigma(scale: usize) -> f64 {
    let fine = smoothing_filter(scale);
    let coarse = smoothing_filter(scale + 1);
    // Both are symmetric about their centres; the finer one sits in the middle of the coarser.
    let offset = (coarse.len() - fine.len()) / 2;
    let fine_energy: f64 = fine.iter().map(|g| g * g).sum();
    let coarse_energy: f64 = coarse.iter().map(|g| g * g).sum();
    let cross: f64 = fine.iter().zip(&coarse[offset..]).map(|(a, b)| a * b).sum();
    (fine_energy * fine_energy - 2.0 * cross * cross + coarse_energy * coarse_energy).sqrt()
}

/// The 1-D kernel `scale` smoothing steps apply: the impulse convolved with the B3 kernel at hole
/// spacings 1, 2, …, `2^(scale−1)`.
fn smoothing_filter(scale: usize) -> Vec<f64> {
    let mut filter = vec![1.0];
    for step in (0..scale).map(|j| 1usize << j) {
        let mut next = vec![0.0; filter.len() + 4 * step];
        for (i, &value) in filter.iter().enumerate() {
            for (tap, &weight) in B3.iter().enumerate() {
                next[i + tap * step] += value * f64::from(weight);
            }
        }
        filter = next;
    }
    filter
}

/// Largest scale count for which the coarsest hole step stays within the image: `2^J ≤ min(w, h)`.
/// Beyond it the à trous kernel spans the whole frame, so the extra scales do nothing.
pub(crate) fn max_scales(size: Size2us) -> usize {
    let min_dim = size.width.min(size.height);
    if min_dim < 2 {
        return 1;
    }
    min_dim.ilog2() as usize
}

#[cfg(feature = "internals")]
pub(crate) mod bench;

#[cfg(test)]
mod tests;
