//! [`MrsNoise`]: the standard deviation of an image's white noise from its multiresolution support.

use std::mem;

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

const TILE: usize = 256;
const TILES_PER_AXIS: usize = 4;
/// Wider than the transform reaches from a pixel: two taps of `2^(j−1)` at each of the four
/// layers, 2 × (1 + 2 + 4 + 8) = 30.
const MARGIN: usize = 32;

/// One analysed pixel: its first detail layer, the largest layer significance `max_j |w_j|/σ_j`,
/// and the image minus the smooth residual.
#[derive(Debug, Clone, Copy)]
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
        sigma(&coefficients(plane, size, &excluded)) as f32
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
        let mut binned = BinnedSamples::new();
        for coefficient in coefficients(plane, size, &excluded) {
            binned.push(bins, gain(coefficient.index), coefficient);
        }
        BackgroundSplit::fit(&binned.measure(|coefficients| sigma(coefficients).powi(2)))
    }
}

/// The coefficients of every tile's core pixels that `excluded` does not name.
fn coefficients(
    plane: &[f32],
    size: Size2us,
    excluded: &(impl Fn(usize) -> bool + Sync),
) -> Vec<Coefficients> {
    debug_assert_eq!(plane.len(), size.pixel_count());
    let scales = SCALES.min(max_scales(size));
    tiles(size)
        .into_par_iter()
        .flat_map_iter(|tile| tile_coefficients(plane, size, tile, scales, excluded))
        .collect()
}

/// The noise standard deviation the support iteration settles on over `coefficients`, corrected
/// for the method's response to white noise; `0` for fewer than two.
fn sigma(coefficients: &[Coefficients]) -> f64 {
    if coefficients.len() < 2 {
        return 0.0;
    }
    let mut sigma = k_sigma(coefficients) / LAYER_SIGMA[0];
    for _ in 0..MAX_ITERATIONS {
        let next = standard_deviation(
            coefficients
                .iter()
                .filter(|c| c.significance < K * sigma)
                .map(|c| c.detail),
        );
        let converged = (next - sigma).abs() <= CONVERGENCE * next;
        sigma = next;
        if converged || sigma == 0.0 {
            break;
        }
    }
    sigma / WHITE_NOISE_RESPONSE
}

/// An analysed tile: the `core` of output pixels within the `region` of the image the transform
/// reads, both as `[x0, x1) × [y0, y1)`.
#[derive(Debug, Clone, Copy)]
struct Tile {
    core: [usize; 4],
    region: [usize; 4],
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
/// name.
fn tile_coefficients(
    plane: &[f32],
    size: Size2us,
    tile: Tile,
    scales: usize,
    excluded: &impl Fn(usize) -> bool,
) -> Vec<Coefficients> {
    let [rx0, rx1, ry0, ry1] = tile.region;
    let (width, height) = (rx1 - rx0, ry1 - ry0);
    let image = Buffer2::new(
        width,
        height,
        (ry0..ry1)
            .flat_map(|y| {
                plane[y * size.width + rx0..y * size.width + rx1]
                    .iter()
                    .copied()
            })
            .collect(),
    );
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
    let mut coefficients = Vec::with_capacity((cx1 - cx0) * (cy1 - cy0));
    for y in cy0..cy1 {
        for x in cx0..cx1 {
            if excluded(y * size.width + x) {
                continue;
            }
            let local = (y - ry0) * width + (x - rx0);
            coefficients.push(Coefficients {
                index: y * size.width + x,
                first_layer: first_layer[local],
                significance: significance[local],
                detail: f64::from(image.pixels()[local]) - f64::from(smooth.pixels()[local]),
            });
        }
    }
    coefficients
}

/// The k-σ clipped standard deviation of the first detail layer: the starting estimate.
fn k_sigma(coefficients: &[Coefficients]) -> f64 {
    let mut sigma = standard_deviation(coefficients.iter().map(|c| c.first_layer));
    for _ in 0..START_ITERATIONS {
        let next = standard_deviation(
            coefficients
                .iter()
                .map(|c| c.first_layer)
                .filter(|w| w.abs() < K * sigma),
        );
        let converged = (next - sigma).abs() <= START_CONVERGENCE * next;
        sigma = next;
        if converged || sigma == 0.0 {
            break;
        }
    }
    sigma
}

/// The sample standard deviation of `values`; `0` for fewer than two.
fn standard_deviation(values: impl Iterator<Item = f64>) -> f64 {
    let (mut count, mut mean, mut m2) = (0.0f64, 0.0f64, 0.0f64);
    for value in values {
        count += 1.0;
        let delta = value - mean;
        mean += delta / count;
        m2 += delta * (value - mean);
    }
    if count < 2.0 {
        0.0
    } else {
        (m2 / (count - 1.0)).sqrt()
    }
}
