//! [`MrsNoise`]: the standard deviation of an image's white noise from its multiresolution support.

use std::mem;

use arrayvec::ArrayVec;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::math::noise::background_split::{BackgroundSplit, BinnedSamples, GainBins};
use crate::math::size2us::Size2us;
use crate::math::wavelet::{atrous_smooth, max_scales};

/// The noise standard deviation of an image by Starck & Murtagh's multiresolution-support method
/// (PASP 110, 1998), as PixInsight's `NoiseMRS` computes it.
///
/// An à trous (starlet) transform splits the image into detail layers `w_j` and a smooth residual
/// `c_J`. A pixel belongs to the support when some layer is significant there, `|w_j| ≥ k·σ·σ_j`,
/// with `σ_j` the layer's response to unit white noise. The noise is the standard deviation of
/// `I − c_J` over the pixels outside the support, iterated from a k-σ clipped first layer until it
/// changes by less than 10⁻⁴ of itself. Stars, nebulae and gradients are structure at some scale,
/// so they leave the estimate, which whole-frame MAD does not do.
///
/// The reference reads white Gaussian noise 2.2% low: the support also removes the pixels where
/// the noise itself passes `k·σ·σ_j`, and `c_J` keeps a little of the noise. That response is a
/// fixed property of `k`, the layer count and the kernel, so it is divided out
/// ([`WHITE_NOISE_RESPONSE`]) and a white noise reads its own σ: the variance plane needs the
/// absolute figure, not only the relative one weights use.
///
/// It runs on a fixed grid of at most 16 tiles of 256², each with a margin wider than the transform
/// reaches, so the memory is bounded whatever the frame size, and each tile's transform equals the
/// whole frame's at its core. A million pixels give the standard deviation to a standard error of
/// 0.07%, far below the method's own bias.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MrsNoise;

/// Layers of the transform; PixInsight's default.
const SCALES: usize = 4;
/// The B3-spline à trous response of each detail layer to unit white noise (Starck & Murtagh).
const LAYER_SIGMA: [f64; SCALES] = [0.889, 0.200, 0.086, 0.041];
/// The significance threshold in units of a layer's noise.
const K: f64 = 3.0;
/// The relative change that ends the iterations, as the reference.
const CONVERGENCE: f64 = 1e-4;
/// What the uncorrected estimate reads for white Gaussian noise of unit σ: 0.97826, the mean over
/// 32 frames of 10⁶ analysed pixels, with a standard error of 1.3 × 10⁻⁴. Not closed-form: the
/// support truncates four correlated layers at once.
const WHITE_NOISE_RESPONSE: f64 = 0.97826;
/// Iterations at most: the reference converges in 4 to 8.
const MAX_ITERATIONS: usize = 50;
/// The k-σ start: its relative accuracy and its iteration cap, as PixInsight's `NoiseKSigma`.
const START_CONVERGENCE: f64 = 0.01;
const START_ITERATIONS: usize = 10;
/// The least share of the analysed pixels an MRS estimate stands on, one in this many, as
/// PixInsight's noise evaluation requires 1%; below it, or with no convergence, the k-σ estimate
/// stands instead.
const MIN_NOISE_SHARE: usize = 100;
/// The first detail layer's response to unit white noise, exactly: `√(1 − 2h₀ + Σh²)` for the 2-D
/// B3 kernel, its centre tap `h₀ = (6/16)²` and `Σh² = (70/256)²`. It holds where the kernel lies
/// inside the frame: within [`FIRST_LAYER_REACH`] of an edge the mirror boundary reads a sample
/// twice. [`LAYER_SIGMA`] holds the literature's three digits for the MRS start, which only seeds
/// the iteration.
const FIRST_LAYER_RESPONSE: f64 = 0.890_796_310_278_758_4;
/// What an iterated 3σ clip reads for a Gaussian of unit σ: the fixed point of
/// `s = √(1 − 2cφ(c)/(2Φ(c) − 1))` at `c = 3s`, the variance of a normal truncated at `±c`,
/// solved by fixed-point iteration.
const K_SIGMA_RESPONSE: f64 = 0.984_846_186_449_778_8;
/// How far the first layer's kernel reaches from its pixel: the B3 taps at ±2.
const FIRST_LAYER_REACH: usize = 2;

const TILE: usize = 256;
const TILES_PER_AXIS: usize = 4;
/// Wider than the transform reaches from a pixel: two taps of `2^(j−1)` at each of the four
/// layers, 2 × (1 + 2 + 4 + 8) = 30.
const MARGIN: usize = 32;
/// The bytes a tile's transform holds for each pixel of its region: the image, the smooth, the
/// next smooth and the convolution's scratch in f32, and the first layer and the significance in
/// f64.
const TILE_BYTES_PER_PIXEL: usize = 4 * size_of::<f32>() + 2 * size_of::<f64>();

/// One analysed pixel: its first detail layer, the largest layer significance `max_j |w_j|/σ_j`,
/// and the image minus the smooth residual.
#[derive(Debug, Clone, Copy, Default)]
struct Coefficients {
    /// The pixel's row-major index in the image.
    index: usize,
    first_layer: f64,
    significance: f64,
    detail: f64,
}

impl MrsNoise {
    /// The noise standard deviation of `plane`, a row-major `size` image, over the pixels
    /// `excluded` does not name; `0` when no pixel remains or the image is constant.
    pub(crate) fn estimate(
        plane: &[f32],
        size: Size2us,
        excluded: impl Fn(usize) -> bool + Sync,
    ) -> f32 {
        let coefficients = coefficients(plane, size, &excluded);
        sigma(&|| coefficients.iter(), size) as f32
    }

    /// The background of `plane` under a flat whose gain at pixel `index` is `gain(index)`: the
    /// noise measured over each of `bins` on its own — the support thresholds against the bin's
    /// own σ — and fitted by [`BackgroundSplit::fit`].
    pub(crate) fn estimate_split(
        plane: &[f32],
        size: Size2us,
        excluded: impl Fn(usize) -> bool + Sync,
        gain: impl Fn(usize) -> f32,
        bins: &GainBins,
    ) -> BackgroundSplit {
        let binned = BinnedSamples::of(coefficients(plane, size, &excluded), bins, |coefficient| {
            gain(coefficient.index)
        });
        BackgroundSplit::fit(
            &binned.measure(|coefficients| sigma(&|| coefficients.iter(), size).powi(2)),
        )
    }

    /// The most bytes an estimate over a `size` image holds at once, with `threads` workers to
    /// share its tiles: [`Self::estimate_split`]'s coefficients, each with its gain bin, the
    /// six planes of every tile in work, the largest first, and the tile list beside them.
    /// [`Self::estimate`] holds no bins and so a byte a coefficient less.
    pub(crate) fn peak_bytes(size: Size2us, threads: usize) -> usize {
        let tiles = tiles(size);
        let analysed: usize = tiles.iter().map(Tile::core_pixels).sum();
        let mut regions: ArrayVec<usize, { TILES_PER_AXIS * TILES_PER_AXIS }> =
            tiles.iter().map(Tile::region_pixels).collect();
        regions.sort_unstable_by(|a, b| b.cmp(a));
        let in_work: usize = regions.iter().take(threads.max(1)).sum();
        BinnedSamples::<Coefficients>::bytes(analysed)
            + in_work * TILE_BYTES_PER_PIXEL
            + tiles.len()
                * (size_of::<Tile>() + size_of::<&mut [Coefficients]>() + size_of::<usize>())
    }
}

/// The coefficients of every tile's core pixels that `excluded` does not name, in tile order and
/// row-major within a tile: each tile writes its core into its own share of one buffer sized for
/// every core pixel, and the shares close up over the pixels excluded.
fn coefficients(
    plane: &[f32],
    size: Size2us,
    excluded: &(impl Fn(usize) -> bool + Sync),
) -> Vec<Coefficients> {
    debug_assert_eq!(plane.len(), size.pixel_count());
    let scales = SCALES.min(max_scales(size));
    let tiles = tiles(size);
    let mut coefficients = vec![Coefficients::default(); tiles.iter().map(Tile::core_pixels).sum()];
    let mut shares: Vec<&mut [Coefficients]> = Vec::with_capacity(tiles.len());
    let mut rest = coefficients.as_mut_slice();
    for tile in &tiles {
        let (share, tail) = rest.split_at_mut(tile.core_pixels());
        shares.push(share);
        rest = tail;
    }
    let mut kept = Vec::with_capacity(tiles.len());
    tiles
        .par_iter()
        .zip(shares)
        .map(|(&tile, share)| tile_coefficients(plane, size, tile, scales, excluded, share))
        .collect_into_vec(&mut kept);
    let (mut start, mut end) = (0, 0);
    for (tile, kept) in tiles.iter().zip(kept) {
        coefficients.copy_within(start..start + kept, end);
        start += tile.core_pixels();
        end += kept;
    }
    coefficients.truncate(end);
    coefficients
}

/// The noise standard deviation the support iteration settles on over the coefficients
/// `coefficients` walks, of a `size` image, corrected for the method's response to white noise;
/// `0` for fewer than two. Where the iteration leaves too few pixels outside the support or does
/// not converge — a small frame whose coarse layers reach every pixel from one bright structure —
/// the k-σ estimate of the first layer over the pixels its kernel lies inside the frame at, run to
/// the same convergence and corrected for its own response; `0` when no two such pixels remain.
fn sigma<'a, I>(coefficients: &impl Fn() -> I, size: Size2us) -> f64
where
    I: Iterator<Item = &'a Coefficients>,
{
    let count = coefficients().count();
    if count < 2 {
        return 0.0;
    }
    if let Some(sigma) = support_sigma(coefficients, count) {
        return sigma / WHITE_NOISE_RESPONSE;
    }
    let reach = FIRST_LAYER_REACH;
    let interior = || {
        coefficients().filter(move |c| {
            let (x, y) = (c.index % size.width, c.index / size.width);
            (reach..size.width.saturating_sub(reach)).contains(&x)
                && (reach..size.height.saturating_sub(reach)).contains(&y)
        })
    };
    tracing::debug!(
        pixels = count,
        interior = interior().count(),
        "the multiresolution support leaves too few noise pixels; using the k-σ estimate"
    );
    k_sigma(&interior, CONVERGENCE, MAX_ITERATIONS) / FIRST_LAYER_RESPONSE / K_SIGMA_RESPONSE
}

/// The support iteration's σ over the `count` coefficients `coefficients` walks, uncorrected;
/// `None` when an iteration leaves fewer than one in [`MIN_NOISE_SHARE`] of them, or two, outside
/// the support, or none converges.
fn support_sigma<'a, I>(coefficients: &impl Fn() -> I, count: usize) -> Option<f64>
where
    I: Iterator<Item = &'a Coefficients>,
{
    let floor = count.div_ceil(MIN_NOISE_SHARE).max(2);
    let mut sigma = k_sigma(coefficients, START_CONVERGENCE, START_ITERATIONS) / LAYER_SIGMA[0];
    for _ in 0..MAX_ITERATIONS {
        let next = Deviation::of(
            coefficients()
                .filter(|c| c.significance < K * sigma)
                .map(|c| c.detail),
        );
        if next.count < floor {
            return None;
        }
        let converged = (next.sigma - sigma).abs() <= CONVERGENCE * next.sigma;
        sigma = next.sigma;
        if converged || sigma == 0.0 {
            return Some(sigma);
        }
    }
    None
}

/// An analysed tile: the `core` of output pixels within the `region` of the image the transform
/// reads, both as `[x0, x1) × [y0, y1)`.
#[derive(Debug, Clone, Copy)]
struct Tile {
    core: [usize; 4],
    region: [usize; 4],
}

impl Tile {
    const fn core_pixels(&self) -> usize {
        let [x0, x1, y0, y1] = self.core;
        (x1 - x0) * (y1 - y0)
    }

    const fn region_pixels(&self) -> usize {
        let [x0, x1, y0, y1] = self.region;
        (x1 - x0) * (y1 - y0)
    }
}

/// Up to [`TILES_PER_AXIS`]² tiles spread evenly over `size`, each core [`TILE`] wide or the whole
/// axis when it is narrower.
fn tiles(size: Size2us) -> Vec<Tile> {
    let spans = |extent: usize| -> Vec<[usize; 2]> {
        let core = TILE.min(extent);
        let count = (extent / TILE).clamp(1, TILES_PER_AXIS);
        (0..count)
            .map(|i| {
                let start = if count == 1 {
                    (extent - core) / 2
                } else {
                    (extent - core) * i / (count - 1)
                };
                [start, start + core]
            })
            .collect()
    };
    let columns = spans(size.width);
    let rows = spans(size.height);
    rows.iter()
        .flat_map(|&[y0, y1]| {
            columns.iter().map(move |&[x0, x1]| Tile {
                core: [x0, x1, y0, y1],
                region: [
                    x0.saturating_sub(MARGIN),
                    (x1 + MARGIN).min(size.width),
                    y0.saturating_sub(MARGIN),
                    (y1 + MARGIN).min(size.height),
                ],
            })
        })
        .collect()
}

/// The transform of one tile, and the coefficients of its core pixels that `excluded` does not
/// name, written to the front of `share`, one place a core pixel; how many it wrote.
fn tile_coefficients(
    plane: &[f32],
    size: Size2us,
    tile: Tile,
    scales: usize,
    excluded: &impl Fn(usize) -> bool,
    share: &mut [Coefficients],
) -> usize {
    debug_assert_eq!(share.len(), tile.core_pixels());
    let [rx0, rx1, ry0, ry1] = tile.region;
    let (width, height) = (rx1 - rx0, ry1 - ry0);
    let mut pixels = Vec::with_capacity(width * height);
    for y in ry0..ry1 {
        pixels.extend_from_slice(&plane[y * size.width + rx0..y * size.width + rx1]);
    }
    let image = Buffer2::new(width, height, pixels);
    let mut smooth = image.clone();
    let mut next = Buffer2::new_default(width, height);
    let mut scratch = Buffer2::new_default(width, height);
    let mut first_layer = vec![0.0f64; width * height];
    let mut significance = vec![0.0f64; width * height];
    for (scale, &layer_sigma) in LAYER_SIGMA.iter().enumerate().take(scales) {
        atrous_smooth(&smooth, &mut next, &mut scratch, 1 << scale);
        for (index, (&coarse, &fine)) in next.pixels().iter().zip(smooth.pixels()).enumerate() {
            let detail = f64::from(fine) - f64::from(coarse);
            if scale == 0 {
                first_layer[index] = detail;
            }
            significance[index] = significance[index].max(detail.abs() / layer_sigma);
        }
        mem::swap(&mut smooth, &mut next);
    }
    let [cx0, cx1, cy0, cy1] = tile.core;
    let mut kept = 0;
    for y in cy0..cy1 {
        for x in cx0..cx1 {
            if excluded(y * size.width + x) {
                continue;
            }
            let local = (y - ry0) * width + (x - rx0);
            share[kept] = Coefficients {
                index: y * size.width + x,
                first_layer: first_layer[local],
                significance: significance[local],
                detail: f64::from(image.pixels()[local]) - f64::from(smooth.pixels()[local]),
            };
            kept += 1;
        }
    }
    kept
}

/// The k-σ clipped standard deviation of the first detail layer of the coefficients
/// `coefficients` walks, iterated until it moves by less than `convergence` of itself or
/// `iterations` run out.
fn k_sigma<'a, I>(coefficients: &impl Fn() -> I, convergence: f64, iterations: usize) -> f64
where
    I: Iterator<Item = &'a Coefficients>,
{
    let mut sigma = Deviation::of(coefficients().map(|c| c.first_layer)).sigma;
    for _ in 0..iterations {
        let next = Deviation::of(
            coefficients()
                .map(|c| c.first_layer)
                .filter(|w| w.abs() < K * sigma),
        )
        .sigma;
        let converged = (next - sigma).abs() <= convergence * next;
        sigma = next;
        if converged || sigma == 0.0 {
            break;
        }
    }
    sigma
}

/// A sample standard deviation and how many values it was taken over.
#[derive(Debug, Clone, Copy)]
struct Deviation {
    sigma: f64,
    count: usize,
}

impl Deviation {
    /// The sample standard deviation of `values`; `0` for fewer than two.
    fn of(values: impl Iterator<Item = f64>) -> Self {
        let (mut count, mut mean, mut m2) = (0usize, 0.0f64, 0.0f64);
        for value in values {
            count += 1;
            let delta = value - mean;
            mean += delta / count as f64;
            m2 += delta * (value - mean);
        }
        let sigma = if count < 2 {
            0.0
        } else {
            (m2 / (count - 1) as f64).sqrt()
        };
        Self { sigma, count }
    }
}

#[cfg(test)]
mod tests {
    use statrs::distribution::{ContinuousCDF, Normal};

    use crate::math::noise::mrs_noise::{FIRST_LAYER_RESPONSE, K, K_SIGMA_RESPONSE};

    /// The first layer's response, `δ − h` for the 2-D B3 kernel `h = b bᵀ` with
    /// `b = [1, 4, 6, 4, 1]/16`, summed in squares tap by tap rather than by the closed form.
    #[test]
    fn the_first_layer_response_is_the_kernels() {
        let b = [1.0, 4.0, 6.0, 4.0, 1.0].map(|tap: f64| tap / 16.0);
        let mut energy = 0.0;
        for (i, row) in b.iter().enumerate() {
            for (j, column) in b.iter().enumerate() {
                let delta = if i == 2 && j == 2 { 1.0 } else { 0.0 };
                energy += (delta - row * column).powi(2);
            }
        }
        assert!((energy.sqrt() - FIRST_LAYER_RESPONSE).abs() < 1e-15);
    }

    /// The clip's response, against the clip itself run on 200 001 evenly spaced quantiles of a
    /// unit normal: a sample with no noise, whose clipped σ differs from the continuous one by its
    /// discretization alone, under 10⁻⁵.
    #[test]
    fn the_clip_response_is_a_clipped_normals() {
        let normal = Normal::standard();
        let count = 200_001u32;
        let values: Vec<f64> = (0..count)
            .map(|i| normal.inverse_cdf((f64::from(i) + 0.5) / f64::from(count)))
            .collect();
        let deviation = |bound: f64| {
            let kept: Vec<f64> = values.iter().copied().filter(|v| v.abs() < bound).collect();
            let mean = kept.iter().sum::<f64>() / kept.len() as f64;
            (kept.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (kept.len() - 1) as f64).sqrt()
        };
        let mut sigma = deviation(f64::INFINITY);
        for _ in 0..200 {
            let next = deviation(K * sigma);
            if next == sigma {
                break;
            }
            sigma = next;
        }
        assert!((sigma - K_SIGMA_RESPONSE).abs() < 1e-5, "{sigma}");
    }
}
