//! [`NoisePlane`]: the standard deviation of an image's white noise from its multiresolution
//! support.

use std::mem;
use std::ops::Range;

use common::{Buffer2, PerWorker, par};
use rayon::prelude::*;

use crate::image::bg_split::BgSplit;
use crate::image::flat_gain::gain_bins::GainBins;
use crate::math::size2us::Size2us;
use crate::math::urect::URect;
use crate::math::vec2us::Vec2us;
use crate::math::wavelet;
use crate::noise::bg_split;
use crate::noise::binned_samples::{BinnedSamples, Samples};

/// Layers of the transform; PixInsight's default.
const SCALES: usize = 4;
/// The B3-spline à trous response of each detail layer to unit white noise (Starck & Murtagh).
const LAYER_SIGMA: [f64; SCALES] = [0.889, 0.200, 0.086, 0.041];
/// The significance threshold in units of a layer's noise.
const K: f64 = 3.0;
/// When the support iterations end, as the reference.
const CONVERGENCE: Convergence = Convergence {
    tolerance: 1e-4,
    iterations: 50,
};
/// What the uncorrected estimate reads for white Gaussian noise of unit σ: 0.97826, the mean over
/// 32 frames of 10⁶ analysed pixels, with a standard error of 1.3 × 10⁻⁴. Not closed-form: the
/// support truncates four correlated layers at once.
const WHITE_NOISE_RESPONSE: f64 = 0.97826;
/// The k-σ start: its relative accuracy and its iteration cap, as PixInsight's `NoiseKSigma`.
const START: Convergence = Convergence {
    tolerance: 0.01,
    iterations: 10,
};
/// The least share of the analysed pixels an MRS estimate stands on, one in this many, as
/// PixInsight's noise evaluation requires 1%; below it, or with no convergence, the k-σ estimate
/// stands instead.
const MIN_NOISE_SHARE: usize = 100;
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
/// The bytes a tile's transform holds for each pixel of its region: the image, the smooth and the
/// next smooth in f32, and the first layer and the significance in f64.
const TILE_BYTES_PER_PIXEL: usize = 3 * size_of::<f32>() + 2 * size_of::<f64>();

/// The planes one tile's transform works in, kept by a worker from one tile to the next: the
/// image, the smooth and the next smooth, the first layer and the significance, and the row each
/// smoothing step convolves its columns into. Each tile refills them at its own region's size.
#[derive(Debug)]
struct TileScratch {
    image: Buffer2<f32>,
    smooth: Buffer2<f32>,
    next: Buffer2<f32>,
    first_layer: Vec<f64>,
    significance: Vec<f64>,
    row: Vec<f32>,
}

impl TileScratch {
    /// Empty planes with room for regions of up to `pixels` and rows of up to `width`, made whole
    /// at once: a plane that grew from a smaller region to a larger one would keep twice the
    /// smaller, past its price.
    fn with_room(pixels: usize, width: usize) -> Self {
        let plane = || Buffer2::new(0, 0, Vec::with_capacity(pixels));
        Self {
            image: plane(),
            smooth: plane(),
            next: plane(),
            first_layer: Vec::with_capacity(pixels),
            significance: Vec::with_capacity(pixels),
            row: Vec::with_capacity(width),
        }
    }
}

/// The analysed pixels, one column for each figure, so that a pass of the support iteration reads
/// only the columns it tests and sums: each pixel's row-major index in the image, its first detail
/// layer, its largest layer significance `max_j |w_j|/σ_j`, and the image minus the smooth
/// residual.
#[derive(Debug, Default)]
struct Coefficients {
    idx: Vec<usize>,
    first_layer: Vec<f64>,
    significance: Vec<f64>,
    detail: Vec<f64>,
}

/// Consecutive pixels of [`Coefficients`], each column over the same run.
#[derive(Debug, Clone, Copy)]
struct CoefficientRun<'a> {
    idx: &'a [usize],
    first_layer: &'a [f64],
    significance: &'a [f64],
    detail: &'a [f64],
}

/// The columns' places one tile writes its core pixels into.
#[derive(Debug)]
struct CoefficientShare<'a> {
    idx: &'a mut [usize],
    first_layer: &'a mut [f64],
    significance: &'a mut [f64],
    detail: &'a mut [f64],
}

impl Coefficients {
    fn zeroed(len: usize) -> Self {
        Self {
            idx: vec![0; len],
            first_layer: vec![0.0; len],
            significance: vec![0.0; len],
            detail: vec![0.0; len],
        }
    }

    fn all(&self) -> CoefficientRun<'_> {
        CoefficientRun {
            idx: &self.idx,
            first_layer: &self.first_layer,
            significance: &self.significance,
            detail: &self.detail,
        }
    }

    /// Each tile's share, in order, as long as its core.
    fn shares(&mut self, tiles: &[Tile]) -> Vec<CoefficientShare<'_>> {
        let mut idx = self.idx.as_mut_slice();
        let mut first_layer = self.first_layer.as_mut_slice();
        let mut significance = self.significance.as_mut_slice();
        let mut detail = self.detail.as_mut_slice();
        let mut shares = Vec::with_capacity(tiles.len());
        for tile in tiles {
            let len = tile.core.area();
            let share;
            (share, idx) = mem::take(&mut idx).split_at_mut(len);
            let (first, rest_first) = mem::take(&mut first_layer).split_at_mut(len);
            let (significant, rest_significance) = mem::take(&mut significance).split_at_mut(len);
            let (residual, rest_detail) = mem::take(&mut detail).split_at_mut(len);
            (first_layer, significance, detail) = (rest_first, rest_significance, rest_detail);
            shares.push(CoefficientShare {
                idx: share,
                first_layer: first,
                significance: significant,
                detail: residual,
            });
        }
        shares
    }

    /// Move the `len` pixels from `from` onwards to `to`, in every column.
    fn move_within(&mut self, from: usize, len: usize, to: usize) {
        self.idx.copy_within(from..from + len, to);
        self.first_layer.copy_within(from..from + len, to);
        self.significance.copy_within(from..from + len, to);
        self.detail.copy_within(from..from + len, to);
    }

    fn truncate(&mut self, len: usize) {
        self.idx.truncate(len);
        self.first_layer.truncate(len);
        self.significance.truncate(len);
        self.detail.truncate(len);
    }
}

impl Samples for Coefficients {
    type Run<'a> = CoefficientRun<'a>;

    const BYTES: usize = size_of::<usize>() + 3 * size_of::<f64>();

    fn len(&self) -> usize {
        self.idx.len()
    }

    fn swap(&mut self, a: usize, b: usize) {
        self.idx.swap(a, b);
        self.first_layer.swap(a, b);
        self.significance.swap(a, b);
        self.detail.swap(a, b);
    }

    fn runs(&mut self, ranges: &[Range<usize>]) -> Vec<CoefficientRun<'_>> {
        ranges
            .iter()
            .map(|range| CoefficientRun {
                idx: &self.idx[range.clone()],
                first_layer: &self.first_layer[range.clone()],
                significance: &self.significance[range.clone()],
                detail: &self.detail[range.clone()],
            })
            .collect()
    }
}

/// The most bytes an estimate over a `size` image holds at once, with `threads` workers to share
/// its tiles: a split's coefficients, each with its gain bin, a [`TileScratch`] for each worker
/// that takes a tile, with room for the largest region's planes and the widest region's row, and
/// the tile list beside them. A tile's transform starts no parallel work, so a worker holds one
/// scratch at a time. An estimate of σ holds no bins and so a byte a coefficient less.
pub(crate) fn heap_bytes(size: Size2us, threads: usize) -> usize {
    let tiles = tiles(size);
    let analysed: usize = tiles.iter().map(|tile| tile.core.area()).sum();
    let workers = threads.max(1).min(tiles.len());
    BinnedSamples::<Coefficients>::heap_bytes(analysed)
        + workers * largest_region(&tiles) * TILE_BYTES_PER_PIXEL
        + workers * widest_region(&tiles) * size_of::<f32>()
        + tiles.len() * (size_of::<Tile>() + size_of::<CoefficientShare<'_>>() + size_of::<usize>())
}

/// The gain a flat applied to a plane: `gain(idx)` at the pixel of row-major `idx`, and the
/// bins its gains fall in.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PixelGains<'a, G> {
    pub(crate) gain: G,
    pub(crate) bins: &'a GainBins,
}

/// A plane whose white noise is estimated, its samples less the pixels `excluded` names, by Starck
/// & Murtagh's multiresolution-support method (PASP 110, 1998), as PixInsight's `NoiseMRS` computes
/// it.
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
#[derive(Debug)]
pub(crate) struct NoisePlane<'a, E> {
    pub(crate) samples: &'a Buffer2<f32>,
    pub(crate) excluded: E,
}

impl<E: Fn(usize) -> bool + Sync> NoisePlane<'_, E> {
    /// The noise standard deviation over the pixels `excluded` does not name; `0` when no pixel
    /// remains or the image is constant.
    pub(crate) fn sigma(&self) -> f32 {
        sigma(self.coefficients().all(), Size2us::from(self.samples)) as f32
    }

    /// The background under the flat `gains` names: the noise measured over each of its bins on
    /// its own — the support thresholds against the bin's own σ — and fitted by
    /// [`bg_split::fit`]; unflattened, of [`Self::sigma`], for a plane no flat divided.
    pub(crate) fn bg(
        &self,
        gains: Option<PixelGains<'_, impl Fn(usize) -> f32 + Sync>>,
    ) -> BgSplit {
        let Some(PixelGains { gain, bins }) = gains else {
            return BgSplit::of_sigma(self.sigma());
        };
        let size = Size2us::from(self.samples);
        let binned = BinnedSamples::of(self.coefficients(), bins, |coefficients, pos| {
            gain(coefficients.idx[pos])
        });
        bg_split::fit(&binned.measure_in_turn(|run| sigma(run, size).powi(2)))
    }

    /// The coefficients of every tile's core pixels that `excluded` does not name, in tile order
    /// and row-major within a tile: each tile writes its core into its own share of columns sized
    /// for every core pixel, and the shares close up over the pixels excluded.
    fn coefficients(&self) -> Coefficients {
        let size = Size2us::from(self.samples);
        let scales = SCALES.min(wavelet::max_scales(size));
        let tiles = tiles(size);
        let mut coefficients =
            Coefficients::zeroed(tiles.iter().map(|tile| tile.core.area()).sum());
        let shares = coefficients.shares(&tiles);
        let mut kept = Vec::with_capacity(tiles.len());
        let (largest, widest) = (largest_region(&tiles), widest_region(&tiles));
        // A scratch is made where it is first used: a loop that splits runs on the workers alone
        // and one of a single tile on its caller alone, so the scratches made are
        // `threads.min(tiles)` at most, as `heap_bytes` charges.
        let scratches = PerWorker::new(|| None);
        par::iter(&tiles)
            .zip(shares)
            .map(|(&tile, mut share)| {
                let mut slot = scratches.get();
                let scratch = slot.get_or_insert_with(|| TileScratch::with_room(largest, widest));
                self.tile_coefficients(tile, scales, &mut share, scratch)
            })
            .collect_into_vec(&mut kept);
        let (mut start, mut end) = (0, 0);
        for (tile, kept) in tiles.iter().zip(kept) {
            coefficients.move_within(start, kept, end);
            start += tile.core.area();
            end += kept;
        }
        coefficients.truncate(end);
        coefficients
    }

    /// The transform of one tile in `scratch`, and the coefficients of its core pixels that
    /// `excluded` does not name, written to the front of `share`, one place a core pixel; how many
    /// it wrote.
    fn tile_coefficients(
        &self,
        tile: Tile,
        scales: usize,
        share: &mut CoefficientShare<'_>,
        scratch: &mut TileScratch,
    ) -> usize {
        let Self {
            samples: plane,
            excluded,
        } = self;
        let TileScratch {
            image,
            smooth,
            next,
            first_layer,
            significance,
            row: smoothing_row,
        } = scratch;
        let size = Size2us::from(*plane);
        debug_assert_eq!(share.idx.len(), tile.core.area());
        let region = tile.region;
        let (width, height) = (region.width(), region.height());
        image.reshape(width, height, 0.0);
        for (row, y) in image
            .pixels_mut()
            .chunks_exact_mut(width)
            .zip(region.min.y..region.max.y)
        {
            row.copy_from_slice(
                &plane[y * size.width + region.min.x..y * size.width + region.max.x],
            );
        }
        smooth.reshape(width, height, 0.0);
        next.reshape(width, height, 0.0);
        // The first layer is built as it is first smoothed, so neither it nor the significance is
        // cleared first: the largest significance of one layer is that layer's.
        wavelet::atrous_smooth_serial(image, smooth, 1, smoothing_row);
        first_layer.clear();
        first_layer.extend(
            smooth
                .pixels()
                .iter()
                .zip(image.pixels())
                .map(|(&coarse, &fine)| f64::from(fine) - f64::from(coarse)),
        );
        significance.clear();
        significance.extend(
            first_layer
                .iter()
                .map(|detail| detail.abs() / LAYER_SIGMA[0]),
        );
        for (scale, &layer_sigma) in LAYER_SIGMA.iter().enumerate().take(scales).skip(1) {
            wavelet::atrous_smooth_serial(smooth, next, 1 << scale, smoothing_row);
            for ((significance, &coarse), &fine) in significance
                .iter_mut()
                .zip(next.pixels())
                .zip(smooth.pixels())
            {
                let detail = f64::from(fine) - f64::from(coarse);
                *significance = significance.max(detail.abs() / layer_sigma);
            }
            mem::swap(smooth, next);
        }
        let core = tile.core;
        let mut kept = 0;
        for y in core.min.y..core.max.y {
            for x in core.min.x..core.max.x {
                if excluded(y * size.width + x) {
                    continue;
                }
                let local = (y - region.min.y) * width + (x - region.min.x);
                share.idx[kept] = y * size.width + x;
                share.first_layer[kept] = first_layer[local];
                share.significance[kept] = significance[local];
                share.detail[kept] =
                    f64::from(image.pixels()[local]) - f64::from(smooth.pixels()[local]);
                kept += 1;
            }
        }
        kept
    }
}

/// The noise standard deviation the support iteration settles on over `coefficients`, of a `size`
/// image, corrected for the method's response to white noise; `0` for fewer than two. Where the
/// iteration leaves too few pixels outside the support or does not converge — a small frame whose
/// coarse layers reach every pixel from one bright structure — the k-σ estimate of the first layer
/// over the pixels its kernel lies inside the frame at, run to the same convergence and corrected
/// for its own response; `0` when no two such pixels remain.
fn sigma(coefficients: CoefficientRun<'_>, size: Size2us) -> f64 {
    let count = coefficients.idx.len();
    if count < 2 {
        return 0.0;
    }
    if let Some(sigma) = support_sigma(coefficients) {
        return sigma / WHITE_NOISE_RESPONSE;
    }
    let reach = FIRST_LAYER_REACH;
    let interior = |pos: usize| {
        let idx = coefficients.idx[pos];
        let (x, y) = (idx % size.width, idx / size.width);
        (reach..size.width.saturating_sub(reach)).contains(&x)
            && (reach..size.height.saturating_sub(reach)).contains(&y)
    };
    tracing::debug!(
        pixels = count,
        interior = (0..count).filter(|&pos| interior(pos)).count(),
        "the multiresolution support leaves too few noise pixels; using the k-σ estimate"
    );
    // The first layer's exact response to unit white noise, where the kernel lies inside the frame:
    // within `FIRST_LAYER_REACH` of an edge the mirror boundary reads a sample twice, and
    // `interior` keeps those pixels out. `LAYER_SIGMA` keeps the literature's three digits, which
    // the support's threshold and `WHITE_NOISE_RESPONSE` were measured with.
    k_sigma(coefficients, &interior, CONVERGENCE) / wavelet::white_noise_sigma(0) / K_SIGMA_RESPONSE
}

/// The support iteration's σ over `coefficients`, uncorrected; `None` when an iteration leaves
/// fewer than one in [`MIN_NOISE_SHARE`] of them, or two, outside the support, or none converges.
fn support_sigma(coefficients: CoefficientRun<'_>) -> Option<f64> {
    let floor = coefficients.idx.len().div_ceil(MIN_NOISE_SHARE).max(2);
    let mut sigma = k_sigma(coefficients, &|_| true, START) / LAYER_SIGMA[0];
    let mut mean = None;
    for _ in 0..CONVERGENCE.iterations {
        let kept = Kept {
            values: coefficients.detail,
            gate: coefficients.significance,
            below: K * sigma,
            within: &|_| true,
        };
        let next = match mean {
            None => kept.deviation(),
            Some(shift) => kept.deviation_around(shift),
        };
        if next.count < floor {
            return None;
        }
        let converged = (next.sigma - sigma).abs() <= CONVERGENCE.tolerance * next.sigma;
        sigma = next.sigma;
        mean = Some(next.mean);
        if converged || sigma == 0.0 {
            return Some(sigma);
        }
    }
    None
}

/// An analysed tile: the `core` of output pixels within the `region` of the image the transform
/// reads.
#[derive(Debug, Clone, Copy)]
struct Tile {
    core: URect,
    region: URect,
}

/// The pixels of the largest of `tiles`' regions; none for no tile.
fn largest_region(tiles: &[Tile]) -> usize {
    tiles
        .iter()
        .map(|tile| tile.region.area())
        .max()
        .unwrap_or(0)
}

/// The width of the widest of `tiles`' regions; none for no tile.
fn widest_region(tiles: &[Tile]) -> usize {
    tiles
        .iter()
        .map(|tile| tile.region.width())
        .max()
        .unwrap_or(0)
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
    let cols = spans(size.width);
    let rows = spans(size.height);
    rows.iter()
        .flat_map(|&[y0, y1]| {
            cols.iter().map(move |&[x0, x1]| Tile {
                core: URect::new(Vec2us::new(x0, y0), Vec2us::new(x1, y1)),
                region: URect::new(
                    Vec2us::new(x0.saturating_sub(MARGIN), y0.saturating_sub(MARGIN)),
                    Vec2us::new(
                        (x1 + MARGIN).min(size.width),
                        (y1 + MARGIN).min(size.height),
                    ),
                ),
            })
        })
        .collect()
}

/// The k-σ clipped standard deviation of the first detail layer of the pixels `within` keeps,
/// iterated until it moves by less than `convergence.tolerance` of itself or its iterations run
/// out.
fn k_sigma(
    coefficients: CoefficientRun<'_>,
    within: &(impl Fn(usize) -> bool + Sync),
    convergence: Convergence,
) -> f64 {
    let layer = coefficients.first_layer;
    let kept = |below: f64| Kept {
        values: layer,
        gate: layer,
        below,
        within,
    };
    let mut deviation = kept(f64::INFINITY).deviation();
    for _ in 0..convergence.iterations {
        let sigma = deviation.sigma;
        deviation = kept(K * sigma).deviation_around(deviation.mean);
        if (deviation.sigma - sigma).abs() <= convergence.tolerance * deviation.sigma
            || deviation.sigma == 0.0
        {
            break;
        }
    }
    deviation.sigma
}

/// When an iteration ends: a step that moves its figure by at most `tolerance` of itself, or
/// `iterations` steps.
#[derive(Debug, Clone, Copy)]
struct Convergence {
    tolerance: f64,
    iterations: usize,
}

/// A sample standard deviation, the mean it was taken about, and how many values it was taken
/// over.
#[derive(Debug, Clone, Copy)]
struct Deviation {
    sigma: f64,
    mean: f64,
    count: usize,
}

impl Deviation {
    fn from_sums(sums: Sums, shift: f64) -> Self {
        if sums.count < 2 {
            return Self {
                sigma: 0.0,
                mean: shift,
                count: sums.count,
            };
        }
        let count = sums.count as f64;
        let variance = (sums.squares - sums.sum * sums.sum / count) / (count - 1.0);
        Self {
            sigma: variance.max(0.0).sqrt(),
            mean: shift + sums.sum / count,
            count: sums.count,
        }
    }
}

/// The values one pass keeps: those at the positions whose `gate` is below `below` in magnitude
/// and that `within` keeps.
#[derive(Debug)]
struct Kept<'a, W> {
    values: &'a [f64],
    gate: &'a [f64],
    below: f64,
    within: &'a W,
}

impl<W: Fn(usize) -> bool + Sync> Kept<'_, W> {
    /// The sample standard deviation of the kept values; `0` for fewer than two. The mean in one
    /// pass, then the deviations from it in a second, their square sum corrected by their sum:
    /// Chan, Golub & LeVeque's two-pass algorithm, which errs less than an update per value and
    /// divides by nothing per value.
    fn deviation(&self) -> Deviation {
        let first = self.sums(0.0);
        if first.count < 2 {
            return Deviation::from_sums(first, 0.0);
        }
        self.deviation_around(first.sum / first.count as f64)
    }

    /// The same in one pass, the deviations taken from `shift` and corrected by their sum, which
    /// is exact whatever the shift and rounds as the two-pass algorithm's second pass does while
    /// the shift is a small part of σ from the mean. An iteration passes the mean the one before it
    /// found, which its own differs from by a sliver of σ.
    fn deviation_around(&self, shift: f64) -> Deviation {
        Deviation::from_sums(self.sums(shift), shift)
    }

    /// The count, sum and square sum of the kept values' offsets from `shift`, in shares of
    /// [`PASS_SHARE`], each summed in four lanes so that no one chain of additions serializes it.
    fn sums(&self, shift: f64) -> Sums {
        debug_assert_eq!(self.values.len(), self.gate.len());
        let mut shares = Vec::with_capacity(self.values.len().div_ceil(PASS_SHARE));
        par::chunks(self.values, PASS_SHARE)
            .zip(par::chunks(self.gate, PASS_SHARE))
            .enumerate()
            .map(|(share, (values, gate))| {
                let start = share * PASS_SHARE;
                Kept {
                    values,
                    gate,
                    below: self.below,
                    within: &|i| (self.within)(start + i),
                }
                .share_sums(shift)
            })
            .collect_into_vec(&mut shares);
        shares.into_iter().fold(Sums::default(), Sums::plus)
    }
}

impl<W: Fn(usize) -> bool> Kept<'_, W> {
    /// [`Self::sums`] over one share, in four lanes.
    fn share_sums(self, shift: f64) -> Sums {
        let Self {
            values,
            gate,
            below,
            within,
        } = self;
        let mut lanes = [Sums::default(); 4];
        let (value_quads, value_rest) = values.as_chunks::<4>();
        let (gate_quads, gate_rest) = gate.as_chunks::<4>();
        for (quad, (values, gates)) in value_quads.iter().zip(gate_quads).enumerate() {
            for (lane, sums) in lanes.iter_mut().enumerate() {
                let kept = gates[lane].abs() < below && within(4 * quad + lane);
                sums.add(kept, values[lane] - shift);
            }
        }
        let start = 4 * value_quads.len();
        for (i, (&value, &gate)) in value_rest.iter().zip(gate_rest).enumerate() {
            lanes[0].add(gate.abs() < below && within(start + i), value - shift);
        }
        lanes.into_iter().fold(Sums::default(), Sums::plus)
    }
}

/// How many values one parallel share of a pass sums: small enough that one gain bin of a split
/// estimate still spreads over every worker. The shares' sums are added in order, so a pass gives
/// the same bits whatever the thread count.
const PASS_SHARE: usize = 1 << 13;

/// One pass's count of the values kept, and the sum and square sum of their offsets from a shift.
#[derive(Debug, Clone, Copy, Default)]
struct Sums {
    count: usize,
    sum: f64,
    squares: f64,
}

impl Sums {
    fn add(&mut self, kept: bool, offset: f64) {
        let offset = if kept { offset } else { 0.0 };
        self.count += usize::from(kept);
        self.sum += offset;
        self.squares += offset * offset;
    }

    fn plus(self, other: Self) -> Self {
        Self {
            count: self.count + other.count,
            sum: self.sum + other.sum,
            squares: self.squares + other.squares,
        }
    }
}

#[cfg(test)]
mod tests {
    use statrs::distribution::{ContinuousCDF, Normal};

    use crate::math::wavelet;
    use crate::noise::mrs_noise::{K, K_SIGMA_RESPONSE};

    /// The first layer's response, `δ − h` for the 2-D B3 kernel `h = b bᵀ` with
    /// `b = [1, 4, 6, 4, 1]/16`, summed in squares tap by tap rather than by the separable form
    /// `white_noise_sigma` takes, which is `√(1 − 2h₀ + Σh²)` with `h₀ = (6/16)²` and
    /// `Σh² = (70/256)²` here.
    #[test]
    fn the_first_layer_response_is_the_kernels() {
        let b = [1.0, 4.0, 6.0, 4.0, 1.0].map(|tap: f64| tap / 16.0);
        let mut energy = 0.0;
        for (i, row) in b.iter().enumerate() {
            for (j, col) in b.iter().enumerate() {
                let delta = if i == 2 && j == 2 { 1.0 } else { 0.0 };
                energy += (delta - row * col).powi(2);
            }
        }
        assert!((energy.sqrt() - wavelet::white_noise_sigma(0)).abs() < 1e-15);
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
