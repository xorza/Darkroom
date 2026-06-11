//! Shared tiled sky-background mesh estimator (SExtractor/SEP style). A [`TileGrid`] divides the
//! image into a grid of boxes and computes one robust sky value + noise per box — per-box ±σ-clip
//! then the crowding-aware Pearson mode `2.5·median − 1.5·mean` (median fallback on skew), with a
//! 3×3 grid median filter — plus natural-cubic-spline coefficients for C²-continuous interpolation.
//!
//! Foundation module (depends only on `math`/`common`): the canonical robust background estimate,
//! reused by `stacking::star_detection::background` (full-res background+noise map for detection)
//! and `background_extraction` (tile-centre samples feeding the gradient surface fit).

use crate::math::statistics::ClippedStats;
use crate::math::statistics::median_f32_mut;
use crate::math::statistics::sigma_clipped_median_mad;
use common::BitBuffer2;
use common::Buffer2;
use common::Vec2us;
use rayon::prelude::*;

/// Maximum samples per tile for statistics computation.
const MAX_TILE_SAMPLES: usize = 1024;

/// Tile statistics computed during background estimation.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TileStats {
    /// Sky level: SExtractor's crowding-aware estimator (Pearson mode, median fallback when
    /// strongly skewed) over the sigma-clip survivors. See [`sextractor_sky`].
    pub sky: f32,
    pub sigma: f32,
}

/// Tile grid with precomputed centers and spline coefficients for interpolation.
#[derive(Debug)]
pub(crate) struct TileGrid {
    stats: Buffer2<TileStats>,
    /// Second derivatives in Y direction for natural cubic spline (sky).
    /// Layout: tiles_x * tiles_y, row-major (same as stats).
    d2y_sky: Vec<f32>,
    /// Second derivatives in Y direction for natural cubic spline (sigma).
    d2y_sigma: Vec<f32>,
    /// Precomputed X-coordinates of tile centers (one per tile column).
    centers_x: Vec<f32>,
    tile_size: usize,
    dimensions: Vec2us,
}

impl TileGrid {
    /// Create an uninitialized TileGrid with preallocated buffers.
    ///
    /// Call `compute` to fill in the tile statistics.
    pub fn new_uninit(width: usize, height: usize, tile_size: usize) -> Self {
        let tiles_x = width.div_ceil(tile_size);
        let tiles_y = height.div_ceil(tile_size);
        let n = tiles_x * tiles_y;

        // Precompute tile center X-coordinates (invariant across rows)
        let centers_x: Vec<f32> = (0..tiles_x)
            .map(|tx| {
                let x_start = tx * tile_size;
                let x_end = (x_start + tile_size).min(width);
                (x_start + x_end) as f32 * 0.5
            })
            .collect();

        Self {
            stats: Buffer2::new_default(tiles_x, tiles_y),
            d2y_sky: vec![0.0; n],
            d2y_sigma: vec![0.0; n],
            centers_x,
            tile_size,
            dimensions: Vec2us::new(width, height),
        }
    }

    /// Compute tile statistics, reusing the existing buffer. `median_filter` applies the 3×3 grid
    /// median filter — keep it on for detection (de-rings a star-contaminated tile before
    /// interpolation); turn it off when fitting a smooth surface to the tile samples, where it would
    /// bias the boundary tiles of a real gradient.
    pub fn compute(
        &mut self,
        pixels: &Buffer2<f32>,
        mask: Option<&BitBuffer2>,
        sigma_clip_iterations: usize,
        median_filter: bool,
    ) {
        debug_assert_eq!(pixels.width(), self.dimensions.x);
        debug_assert_eq!(pixels.height(), self.dimensions.y);

        self.fill_tile_stats(pixels, mask, sigma_clip_iterations);
        if median_filter {
            self.apply_median_filter();
        }
        self.compute_y_spline_derivatives();
    }

    #[inline]
    pub fn get(&self, tx: usize, ty: usize) -> TileStats {
        self.stats[(tx, ty)]
    }

    #[inline]
    pub fn tiles_x(&self) -> usize {
        self.stats.width()
    }

    #[inline]
    pub fn tiles_y(&self) -> usize {
        self.stats.height()
    }

    /// Precomputed X-coordinates of all tile centers.
    #[inline]
    pub fn centers_x(&self) -> &[f32] {
        &self.centers_x
    }

    #[inline]
    pub fn center_y(&self, ty: usize) -> f32 {
        let y_start = ty * self.tile_size;
        let y_end = (y_start + self.tile_size).min(self.dimensions.y);
        (y_start + y_end) as f32 * 0.5
    }

    /// Second derivative of sky in Y at tile (tx, ty) for natural cubic spline.
    #[inline]
    pub fn d2y_sky(&self, tx: usize, ty: usize) -> f32 {
        self.d2y_sky[ty * self.tiles_x() + tx]
    }

    /// Second derivative of sigma in Y at tile (tx, ty) for natural cubic spline.
    #[inline]
    pub fn d2y_sigma(&self, tx: usize, ty: usize) -> f32 {
        self.d2y_sigma[ty * self.tiles_x() + tx]
    }

    /// Find the tile index whose center is at or before the given Y position.
    #[inline]
    pub fn find_lower_tile_y(&self, pos: f32) -> usize {
        // tiles_y >= 1 always (the grid is built from an image with at least one tile row).
        let tiles_y = self.tiles_y();

        // Binary search for largest tile index with center <= pos
        let mut lo = 0;
        let mut hi = tiles_y;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.center_y(mid) <= pos {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo.saturating_sub(1)
    }

    fn fill_tile_stats(
        &mut self,
        pixels: &Buffer2<f32>,
        mask: Option<&BitBuffer2>,
        sigma_clip_iterations: usize,
    ) {
        let tiles_x = self.tiles_x();
        let tile_size = self.tile_size;
        let width = self.dimensions.x;
        let height = self.dimensions.y;
        let max_tile_pixels = tile_size * tile_size;

        self.stats
            .pixels_mut()
            .par_iter_mut()
            .enumerate()
            .for_each_init(
                || {
                    (
                        Vec::with_capacity(max_tile_pixels),
                        Vec::with_capacity(max_tile_pixels),
                    )
                },
                |(values, deviations), (idx, out)| {
                    let tx = idx % tiles_x;
                    let ty = idx / tiles_x;

                    let x_start = tx * tile_size;
                    let y_start = ty * tile_size;
                    let x_end = (x_start + tile_size).min(width);
                    let y_end = (y_start + tile_size).min(height);

                    *out = compute_tile_stats(
                        pixels,
                        mask,
                        x_start,
                        x_end,
                        y_start,
                        y_end,
                        sigma_clip_iterations,
                        values,
                        deviations,
                    );
                },
            );
    }

    fn apply_median_filter(&mut self) {
        let tiles_x = self.tiles_x();
        let tiles_y = self.tiles_y();

        if tiles_x < 3 || tiles_y < 3 {
            return;
        }

        let src = self.stats.pixels();
        let mut dst: Buffer2<TileStats> = Buffer2::new_default(tiles_x, tiles_y);

        dst.pixels_mut()
            .par_iter_mut()
            .enumerate()
            .for_each(|(idx, out)| {
                let tx = idx % tiles_x;
                let ty = idx / tiles_x;

                let mut skies = [0.0f32; 9];
                let mut sigmas = [0.0f32; 9];
                let mut count = 0;

                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let nx = tx as i32 + dx;
                        let ny = ty as i32 + dy;

                        if nx >= 0 && nx < tiles_x as i32 && ny >= 0 && ny < tiles_y as i32 {
                            let neighbor = src[ny as usize * tiles_x + nx as usize];
                            skies[count] = neighbor.sky;
                            sigmas[count] = neighbor.sigma;
                            count += 1;
                        }
                    }
                }

                out.sky = median_f32_mut(&mut skies[..count]);
                out.sigma = median_f32_mut(&mut sigmas[..count]);
            });

        std::mem::swap(&mut self.stats, &mut dst);
    }

    /// Precompute second derivatives in Y for natural cubic spline interpolation.
    ///
    /// For each tile column (tx), solves a tridiagonal system to find d²f/dy²
    /// at each tile center. Natural boundary conditions: d²f=0 at endpoints.
    fn compute_y_spline_derivatives(&mut self) {
        let tiles_x = self.tiles_x();
        let tiles_y = self.tiles_y();

        if tiles_y < 2 {
            // 0 or 1 tile rows: no spline needed, derivatives stay zero
            return;
        }

        // Collect Y centers
        let centers_y: Vec<f32> = (0..tiles_y).map(|ty| self.center_y(ty)).collect();

        // Scratch buffers for tridiagonal solver (reused per column)
        let mut values = vec![0.0f32; tiles_y];
        let mut d2 = vec![0.0f32; tiles_y];
        let mut scratch = vec![0.0f32; tiles_y.saturating_sub(2)];

        for tx in 0..tiles_x {
            // Solve for sky
            for (ty, val) in values.iter_mut().enumerate() {
                *val = self.stats[(tx, ty)].sky;
            }
            solve_natural_spline_d2(&values, &centers_y, &mut d2, &mut scratch);
            for (ty, &d) in d2.iter().enumerate() {
                self.d2y_sky[ty * tiles_x + tx] = d;
            }

            // Solve for sigma
            for (ty, val) in values.iter_mut().enumerate() {
                *val = self.stats[(tx, ty)].sigma;
            }
            solve_natural_spline_d2(&values, &centers_y, &mut d2, &mut scratch);
            for (ty, &d) in d2.iter().enumerate() {
                self.d2y_sigma[ty * tiles_x + tx] = d;
            }
        }
    }
}

/// Evaluate natural cubic spline between two nodes.
///
/// Given function values `f0`, `f1` and second derivatives `d0`, `d1` at the
/// endpoints of an interval of width `h`, evaluates the cubic at parameter
/// `t` in [0, 1] (where t=0 gives f0, t=1 gives f1).
///
/// Standard cubic spline formula (Numerical Recipes, SEP/SExtractor):
///   f(t) = (1-t)*f0 + t*f1 + ((1-t)³ - (1-t))*a + (t³ - t)*b
/// where a = h²/6 * d2_0, b = h²/6 * d2_1.
///
/// Factored form (since (ct³-ct) = -t*ct*(2-t) and (t³-t) = -t*ct*(1+t)):
///   f(t) = (1-t)*f0 + t*f1 - t*(1-t)*((2-t)*a + (1+t)*b)
#[inline]
pub(crate) fn cubic_spline_eval(f0: f32, f1: f32, d0: f32, d1: f32, h: f32, t: f32) -> f32 {
    if h <= 0.0 {
        return f0;
    }
    let h2_6 = h * h / 6.0;
    let a = h2_6 * d0;
    let b = h2_6 * d1;
    let ct = 1.0 - t;
    let t_ct = t * ct;
    ct * f0 + t * f1 - t_ct * ((2.0 - t) * a + (1.0 + t) * b)
}

/// Solve for second derivatives of a natural cubic spline.
///
/// Given `n` function values at positions `centers`, computes the second
/// derivatives `d2[0..n]` using a tridiagonal solver with natural boundary
/// conditions (d2[0] = d2[n-1] = 0).
///
/// `scratch` must have length >= `n - 2` (used for modified upper diagonal
/// coefficients in the Thomas algorithm). Pass a reusable buffer to avoid
/// per-call heap allocation.
///
/// Supports non-uniform spacing. O(n) forward elimination + back substitution.
pub(crate) fn solve_natural_spline_d2(
    values: &[f32],
    centers: &[f32],
    d2: &mut [f32],
    scratch: &mut [f32],
) {
    let n = values.len();
    debug_assert_eq!(centers.len(), n);
    debug_assert!(d2.len() >= n);

    if n < 3 {
        // With < 3 points, natural spline has d2 = 0 everywhere
        d2[..n].fill(0.0);
        return;
    }

    // Interval spacings: h[i] = centers[i+1] - centers[i]
    // For n points, we have n-1 intervals and n-2 interior equations.
    //
    // The tridiagonal system for interior points i = 1..n-2:
    //   h[i-1] * d2[i-1] + 2*(h[i-1]+h[i]) * d2[i] + h[i] * d2[i+1]
    //     = 6 * ((f[i+1]-f[i])/h[i] - (f[i]-f[i-1])/h[i-1])
    //
    // With natural BC: d2[0] = 0, d2[n-1] = 0.
    // This reduces to (n-2) equations for d2[1..n-2].

    let m = n - 2; // number of interior unknowns
    debug_assert!(scratch.len() >= m);

    // Forward elimination (Thomas algorithm)
    // We store modified diagonal and RHS in d2[] (reusing output buffer)
    // and use `scratch` for the modified upper diagonal.
    let cp = &mut scratch[..m];

    // First interior equation (i=1):
    let h0 = centers[1] - centers[0];
    let h1 = centers[2] - centers[1];
    let diag = 2.0 * (h0 + h1);
    let rhs = 6.0 * ((values[2] - values[1]) / h1 - (values[1] - values[0]) / h0);

    cp[0] = h1 / diag;
    d2[1] = rhs / diag;

    // Forward sweep for remaining interior equations
    for k in 1..m {
        let i = k + 1; // actual tile index
        let h_prev = centers[i] - centers[i - 1];
        let h_curr = centers[i + 1] - centers[i];
        let d = 2.0 * (h_prev + h_curr);
        let r = 6.0 * ((values[i + 1] - values[i]) / h_curr - (values[i] - values[i - 1]) / h_prev);

        let denom = d - h_prev * cp[k - 1];
        cp[k] = h_curr / denom;
        d2[i] = (r - h_prev * d2[i - 1]) / denom;
    }

    // Back substitution
    for k in (0..m - 1).rev() {
        let i = k + 1;
        d2[i] -= cp[k] * d2[i + 1];
    }

    // Natural boundary conditions
    d2[0] = 0.0;
    d2[n - 1] = 0.0;
}

/// Compute sigma-clipped statistics for a tile region.
///
/// When a mask is provided, only unmasked pixels are used. If all pixels
/// are masked, falls back to sampling all pixels (including masked) as a
/// last resort. A noisy estimate from few background pixels is far better
/// than a biased estimate contaminated by star flux.
#[allow(clippy::too_many_arguments)]
fn compute_tile_stats(
    pixels: &Buffer2<f32>,
    mask: Option<&BitBuffer2>,
    x_start: usize,
    x_end: usize,
    y_start: usize,
    y_end: usize,
    sigma_clip_iterations: usize,
    values: &mut Vec<f32>,
    deviations: &mut Vec<f32>,
) -> TileStats {
    values.clear();

    let width = pixels.width();
    let tile_pixels = (x_end - x_start) * (y_end - y_start);

    match mask {
        Some(m) => {
            collect_unmasked_pixels(pixels, m, x_start, x_end, y_start, y_end, width, values);

            if values.is_empty() {
                // All pixels masked — no choice but to use all pixels
                collect_sampled_pixels(pixels, x_start, x_end, y_start, y_end, width, values);
            } else if values.len() > MAX_TILE_SAMPLES {
                subsample_in_place(values, MAX_TILE_SAMPLES);
            }
        }
        None => {
            if tile_pixels <= MAX_TILE_SAMPLES {
                collect_all_pixels(pixels, x_start, x_end, y_start, y_end, width, values);
            } else {
                collect_sampled_pixels(pixels, x_start, x_end, y_start, y_end, width, values);
            }
        }
    }

    if values.is_empty() {
        return TileStats::default();
    }

    let stats = sigma_clipped_median_mad(values, deviations, 3.0, sigma_clip_iterations);

    TileStats {
        sky: sextractor_sky(&stats),
        sigma: stats.sigma,
    }
}

/// SExtractor's crowding-aware sky estimator (Bertin & Arnouts 1996, `back.c`).
///
/// Even after clipping, the sky histogram keeps a bright-ward tail from faint sources, so
/// `mean > median > mode` and the median alone systematically over-estimates the sky. Pearson's
/// empirical mode `2.5·median − 1.5·mean` cancels that residual skew. When the tile is strongly
/// skewed (crowded: `|mean − median| ≥ 0.3·σ`) the extrapolation becomes unreliable, so it falls
/// back to the plain median. σ = 0 also takes the fallback — the clip couldn't separate outliers
/// there (zero spread estimate), so the mean is untrustworthy while the median stays robust.
fn sextractor_sky(stats: &ClippedStats) -> f32 {
    if (stats.mean - stats.median).abs() < 0.3 * stats.sigma {
        2.5 * stats.median - 1.5 * stats.mean
    } else {
        stats.median
    }
}

/// Collect all pixels from a tile region.
#[inline]
fn collect_all_pixels(
    pixels: &Buffer2<f32>,
    x_start: usize,
    x_end: usize,
    y_start: usize,
    y_end: usize,
    width: usize,
    values: &mut Vec<f32>,
) {
    let tile_width = x_end - x_start;
    for y in y_start..y_end {
        let row_start = y * width + x_start;
        values.extend_from_slice(&pixels[row_start..row_start + tile_width]);
    }
}

/// Collect sampled pixels using strided access (~MAX_TILE_SAMPLES pixels).
#[inline]
fn collect_sampled_pixels(
    pixels: &Buffer2<f32>,
    x_start: usize,
    x_end: usize,
    y_start: usize,
    y_end: usize,
    width: usize,
    values: &mut Vec<f32>,
) {
    let tile_pixels = (x_end - x_start) * (y_end - y_start);
    let stride = ((tile_pixels / MAX_TILE_SAMPLES).max(1) as f32)
        .sqrt()
        .ceil() as usize;

    for y in (y_start..y_end).step_by(stride) {
        let row_start = y * width;
        for x in (x_start..x_end).step_by(stride) {
            values.push(pixels[row_start + x]);
        }
    }
}

/// Collect unmasked pixels using word-level bit operations.
#[inline]
#[allow(clippy::too_many_arguments)]
fn collect_unmasked_pixels(
    pixels: &Buffer2<f32>,
    mask: &BitBuffer2,
    x_start: usize,
    x_end: usize,
    y_start: usize,
    y_end: usize,
    width: usize,
    values: &mut Vec<f32>,
) {
    let mask_words = mask.words();
    let words_per_row = mask.words_per_row();

    for y in y_start..y_end {
        let row_start = y * width;
        let word_row_start = y * words_per_row;
        let mut x = x_start;

        while x < x_end {
            let word_idx = x / 64;
            let bit_offset = x % 64;
            let mask_word = mask_words[word_row_start + word_idx];

            let bits_in_word = 64 - bit_offset;
            let bits_to_process = bits_in_word.min(x_end - x);

            let relevant_bits = if bits_to_process == 64 {
                !0u64
            } else {
                ((1u64 << bits_to_process) - 1) << bit_offset
            };

            let unmasked = !mask_word & relevant_bits;

            if unmasked != 0 {
                let mut bits = unmasked >> bit_offset;
                let mut local_x = x;

                while bits != 0 && local_x < x_end {
                    let offset = bits.trailing_zeros() as usize;
                    local_x = x + offset;

                    if local_x < x_end {
                        values.push(pixels[row_start + local_x]);
                    }
                    bits &= bits - 1;
                }
            }

            x += bits_to_process;
        }
    }
}

/// Subsample a vector in place to exactly `target_size` elements, evenly spread across the input.
///
/// Picks `read_idx = i * len / target_size` so the kept samples span the whole tile rather than its
/// first rows (an integer `len / target_size` stride collapses to 1 for `len` just above the target,
/// keeping only the leading run). `read_idx >= write_idx` always (since `len > target_size`), so the
/// in-place overwrite never clobbers an unread element.
#[inline]
fn subsample_in_place(values: &mut Vec<f32>, target_size: usize) {
    let len = values.len();
    if len <= target_size {
        return;
    }

    for write_idx in 0..target_size {
        let read_idx = write_idx * len / target_size;
        values[write_idx] = values[read_idx];
    }

    values.truncate(target_size);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Number of sigma-clipping iterations for tests.
    const TEST_SIGMA_CLIP_ITERATIONS: usize = 2;

    fn create_uniform_image(width: usize, height: usize, value: f32) -> Buffer2<f32> {
        Buffer2::new(width, height, vec![value; width * height])
    }

    /// Create a TileGrid with default test parameters (no mask, default sigma clip iterations)
    fn make_grid(pixels: &Buffer2<f32>, tile_size: usize) -> TileGrid {
        let mut grid = TileGrid::new_uninit(pixels.width(), pixels.height(), tile_size);
        grid.compute(pixels, None, TEST_SIGMA_CLIP_ITERATIONS, true);
        grid
    }

    /// Create a TileGrid with mask
    fn make_grid_with_mask(pixels: &Buffer2<f32>, tile_size: usize, mask: &BitBuffer2) -> TileGrid {
        let mut grid = TileGrid::new_uninit(pixels.width(), pixels.height(), tile_size);
        grid.compute(pixels, Some(mask), TEST_SIGMA_CLIP_ITERATIONS, true);
        grid
    }

    // --- SExtractor sky estimator ---

    #[test]
    fn sextractor_sky_hand_computed() {
        let stats = |median: f32, mean: f32, sigma: f32| ClippedStats {
            median,
            sigma,
            mean,
        };
        // Mild skew (|mean−median| = 0.2 < 0.3σ): Pearson mode 2.5·100 − 1.5·100.2 = 99.7,
        // pulled below the median toward the histogram peak.
        let sky = sextractor_sky(&stats(100.0, 100.2, 1.0));
        assert!((sky - 99.7).abs() < 1e-4, "mode = 99.7, got {sky}");
        // Strong skew (1.0 ≥ 0.3σ): the mode extrapolation is unreliable → plain median.
        assert_eq!(sextractor_sky(&stats(100.0, 101.0, 1.0)), 100.0);
        // Symmetric histogram: mode = 2.5·m − 1.5·m = m — estimator changes nothing.
        assert_eq!(sextractor_sky(&stats(100.0, 100.0, 1.0)), 100.0);
        // Uniform tile (σ = 0): |0| < 0 is false → median fallback.
        assert_eq!(sextractor_sky(&stats(5.0, 5.0, 0.0)), 5.0);
    }

    #[test]
    fn skewed_tile_sky_sits_below_median() {
        // One 32×32 tile: a symmetric ramp 0.1 + i·1e-5 (i = 0..1024) whose top 200 values get
        // +0.005 — a bright-ward tail that survives 3σ clipping (max deviation ≈ 0.0101 < 3σ ≈
        // 0.0114). Hand-computed: median ≈ 0.10512 (unchanged by shifting the top values),
        // mean = median + 200·0.005/1024 ≈ median + 0.00098, |mean−median| < 0.3σ → mode fires:
        //   sky = 2.5·0.10512 − 1.5·0.10610 ≈ 0.10365
        // — below the median-only estimate by ~1.5e-3.
        let n = 1024usize;
        let mut values: Vec<f32> = (0..n).map(|i| 0.1 + i as f32 * 1e-5).collect();
        for v in values.iter_mut().skip(n - 200) {
            *v += 0.005;
        }
        let pixels = Buffer2::new(32, 32, values);
        let grid = make_grid(&pixels, 32);
        let sky = grid.get(0, 0).sky;
        assert!(
            (sky - 0.10365).abs() < 5e-4,
            "Pearson-mode sky ≈ 0.10365, got {sky}"
        );
        assert!(
            sky < 0.1045,
            "sky must sit below the median-only estimate (≈0.10512), got {sky}"
        );
    }

    // --- Construction ---

    #[test]
    fn test_tile_grid_dimensions() {
        let pixels = create_uniform_image(128, 64, 0.5);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.tiles_x(), 4);
        assert_eq!(grid.tiles_y(), 2);
    }

    #[test]
    fn test_tile_grid_dimensions_non_divisible() {
        let pixels = create_uniform_image(100, 70, 0.5);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.tiles_x(), 4);
        assert_eq!(grid.tiles_y(), 3);
    }

    #[test]
    fn test_tile_grid_uniform_image() {
        let pixels = create_uniform_image(64, 64, 0.3);
        let grid = make_grid(&pixels, 32);

        for ty in 0..grid.tiles_y() {
            for tx in 0..grid.tiles_x() {
                let stats = grid.get(tx, ty);
                assert!((stats.sky - 0.3).abs() < 0.01);
                assert!(stats.sigma < 0.01);
            }
        }
    }

    // --- Center computation ---

    #[test]
    fn test_center_x_full_tiles() {
        let pixels = create_uniform_image(128, 64, 0.5);
        let grid = make_grid(&pixels, 32);

        assert!((grid.centers_x()[0] - 16.0).abs() < 0.01);
        assert!((grid.centers_x()[1] - 48.0).abs() < 0.01);
        assert!((grid.centers_x()[2] - 80.0).abs() < 0.01);
        assert!((grid.centers_x()[3] - 112.0).abs() < 0.01);
    }

    #[test]
    fn test_center_x_partial_tile() {
        let pixels = create_uniform_image(100, 64, 0.5);
        let grid = make_grid(&pixels, 32);

        assert!((grid.centers_x()[3] - 98.0).abs() < 0.01);
    }

    #[test]
    fn test_center_y_full_tiles() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = make_grid(&pixels, 32);

        assert!((grid.center_y(0) - 16.0).abs() < 0.01);
        assert!((grid.center_y(1) - 48.0).abs() < 0.01);
        assert!((grid.center_y(2) - 80.0).abs() < 0.01);
        assert!((grid.center_y(3) - 112.0).abs() < 0.01);
    }

    #[test]
    fn test_center_y_partial_tile() {
        let pixels = create_uniform_image(64, 100, 0.5);
        let grid = make_grid(&pixels, 32);

        assert!((grid.center_y(3) - 98.0).abs() < 0.01);
    }

    // --- find_lower_tile_y ---

    #[test]
    fn test_find_lower_tile_y_exact_center() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.find_lower_tile_y(16.0), 0);
        assert_eq!(grid.find_lower_tile_y(48.0), 1);
        assert_eq!(grid.find_lower_tile_y(80.0), 2);
        assert_eq!(grid.find_lower_tile_y(112.0), 3);
    }

    #[test]
    fn test_find_lower_tile_y_between_centers() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.find_lower_tile_y(30.0), 0);
        assert_eq!(grid.find_lower_tile_y(60.0), 1);
        assert_eq!(grid.find_lower_tile_y(100.0), 2);
    }

    #[test]
    fn test_find_lower_tile_y_before_first_center() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.find_lower_tile_y(0.0), 0);
        assert_eq!(grid.find_lower_tile_y(10.0), 0);
    }

    #[test]
    fn test_find_lower_tile_y_after_last_center() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.find_lower_tile_y(120.0), 3);
        assert_eq!(grid.find_lower_tile_y(1000.0), 3);
    }

    #[test]
    fn test_find_lower_tile_y_single_tile() {
        let pixels = create_uniform_image(32, 32, 0.5);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.tiles_y(), 1);
        assert_eq!(grid.find_lower_tile_y(0.0), 0);
        assert_eq!(grid.find_lower_tile_y(16.0), 0);
        assert_eq!(grid.find_lower_tile_y(100.0), 0);
    }

    // --- Mask handling ---

    #[test]
    fn test_tile_grid_with_mask_excludes_masked() {
        let width = 64;
        let height = 64;
        let mut data = vec![0.2; width * height];

        for y in 0..32 {
            for x in 0..32 {
                data[y * width + x] = 0.8;
            }
        }

        let pixels = Buffer2::new(width, height, data);

        let mut mask = BitBuffer2::new_filled(width, height, false);
        for y in 0..32 {
            for x in 0..32 {
                mask.set_xy(x, y, true);
            }
        }

        let grid = make_grid_with_mask(&pixels, 32, &mask);

        let stats_11 = grid.get(1, 1);
        assert!((stats_11.sky - 0.2).abs() < 0.05);
    }

    #[test]
    fn test_tile_uses_few_unmasked_pixels_over_all_pixels() {
        // Tile (0,0) has 95% masked "star" pixels at 0.9, 5% unmasked background at 0.2.
        // The unmasked pixels should be used for background estimation (median ≈ 0.2),
        // NOT falling back to all pixels which would give a biased median toward 0.9.
        let width = 64;
        let height = 64;

        // Start with all pixels at "star" value
        let mut data = vec![0.9f32; width * height];

        // Set ~5% of the top-left tile (32×32 = 1024 pixels) to background value
        // 5% of 1024 = ~51 pixels. Use a stripe: first 2 rows unmasked.
        // 2 rows × 32 cols = 64 pixels of background
        let mut mask = BitBuffer2::new_filled(width, height, false);
        for y in 0..32 {
            for x in 0..32 {
                if y < 2 {
                    // Background pixels: unmasked, value 0.2
                    data[y * width + x] = 0.2;
                    // mask stays false (unmasked)
                } else {
                    // Star pixels: masked, value 0.9
                    mask.set_xy(x, y, true);
                }
            }
        }

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid_with_mask(&pixels, 32, &mask);

        let stats = grid.get(0, 0);
        // With the fix: uses the 64 unmasked background pixels → median ≈ 0.2
        // Without the fix: falls back to all 1024 pixels → median biased toward 0.9
        assert!(
            (stats.sky - 0.2).abs() < 0.05,
            "Tile (0,0) median should be ~0.2 (background), got {}",
            stats.sky
        );
    }

    #[test]
    fn test_all_pixels_masked_fallback() {
        let width = 64;
        let height = 64;
        let pixels = create_uniform_image(width, height, 0.4);
        let mask = BitBuffer2::new_filled(width, height, true);

        let grid = make_grid_with_mask(&pixels, 32, &mask);

        let stats = grid.get(0, 0);
        assert!((stats.sky - 0.4).abs() < 0.05);
    }

    #[test]
    fn test_no_mask_same_as_none() {
        let pixels = create_uniform_image(64, 64, 0.5);

        let grid_none = make_grid(&pixels, 32);

        let mut grid_empty = TileGrid::new_uninit(64, 64, 32);
        grid_empty.compute(&pixels, None, TEST_SIGMA_CLIP_ITERATIONS, true);

        for ty in 0..grid_none.tiles_y() {
            for tx in 0..grid_none.tiles_x() {
                let s1 = grid_none.get(tx, ty);
                let s2 = grid_empty.get(tx, ty);
                assert!((s1.sky - s2.sky).abs() < 0.001);
            }
        }
    }

    // --- Median filter ---

    #[test]
    fn test_median_filter_uniform_unchanged() {
        let pixels = create_uniform_image(128, 128, 0.4);
        let grid = make_grid(&pixels, 32);

        for ty in 0..grid.tiles_y() {
            for tx in 0..grid.tiles_x() {
                let stats = grid.get(tx, ty);
                assert!((stats.sky - 0.4).abs() < 0.01);
            }
        }
    }

    #[test]
    fn test_median_filter_rejects_outlier_tile() {
        let width = 128;
        let height = 128;
        let mut data = vec![0.3; width * height];

        for y in 32..64 {
            for x in 32..64 {
                data[y * width + x] = 0.9;
            }
        }

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 32);

        let center_stats = grid.get(1, 1);
        assert!((center_stats.sky - 0.3).abs() < 0.1);
    }

    #[test]
    fn test_median_filter_skipped_for_small_grid() {
        let pixels = create_uniform_image(64, 64, 0.5);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.tiles_x(), 2);
        assert_eq!(grid.tiles_y(), 2);

        let stats = grid.get(0, 0);
        assert!((stats.sky - 0.5).abs() < 0.01);
    }

    // --- Edge cases ---

    #[test]
    fn test_single_tile_image() {
        let pixels = create_uniform_image(32, 32, 0.6);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.tiles_x(), 1);
        assert_eq!(grid.tiles_y(), 1);

        let stats = grid.get(0, 0);
        assert!((stats.sky - 0.6).abs() < 0.01);
        assert!((grid.centers_x()[0] - 16.0).abs() < 0.01);
        assert!((grid.center_y(0) - 16.0).abs() < 0.01);
    }

    #[test]
    fn test_tile_stats_with_gradient() {
        let width = 64;
        let height = 64;
        let data: Vec<f32> = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x + y) as f32 / 128.0))
            .collect();

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 32);

        let tl = grid.get(0, 0);
        let br = grid.get(1, 1);
        assert!(br.sky > tl.sky);
    }

    #[test]
    fn test_debug_impl() {
        let pixels = create_uniform_image(64, 64, 0.5);
        let grid = make_grid(&pixels, 32);

        let debug_str = format!("{:?}", grid);
        assert!(debug_str.contains("TileGrid"));
    }

    #[test]
    fn test_image_smaller_than_tile() {
        let pixels = create_uniform_image(20, 20, 0.7);
        let grid = make_grid(&pixels, 64);

        assert_eq!(grid.tiles_x(), 1);
        assert_eq!(grid.tiles_y(), 1);

        let stats = grid.get(0, 0);
        assert!((stats.sky - 0.7).abs() < 0.01);
        assert!((grid.centers_x()[0] - 10.0).abs() < 0.01);
        assert!((grid.center_y(0) - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_large_tile_size() {
        let pixels = create_uniform_image(100, 50, 0.3);
        let grid = make_grid(&pixels, 200);

        assert_eq!(grid.tiles_x(), 1);
        assert_eq!(grid.tiles_y(), 1);

        let stats = grid.get(0, 0);
        assert!((stats.sky - 0.3).abs() < 0.01);
    }

    #[test]
    fn test_tile_grid_very_wide_image() {
        let pixels = create_uniform_image(1000, 10, 0.5);
        let grid = make_grid(&pixels, 64);

        assert_eq!(grid.tiles_x(), 16);
        assert_eq!(grid.tiles_y(), 1);

        for tx in 0..grid.tiles_x() {
            let stats = grid.get(tx, 0);
            assert!((stats.sky - 0.5).abs() < 0.01);
        }
    }

    #[test]
    fn test_tile_grid_very_tall_image() {
        let pixels = create_uniform_image(10, 1000, 0.5);
        let grid = make_grid(&pixels, 64);

        assert_eq!(grid.tiles_x(), 1);
        assert_eq!(grid.tiles_y(), 16);

        for ty in 0..grid.tiles_y() {
            let stats = grid.get(0, ty);
            assert!((stats.sky - 0.5).abs() < 0.01);
        }
    }

    // --- Helper function tests ---

    #[test]
    fn test_subsample_in_place_no_change_when_small() {
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        subsample_in_place(&mut values, 10);
        assert_eq!(values.len(), 5);
    }

    #[test]
    fn test_subsample_in_place_reduces_size() {
        let mut values: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        subsample_in_place(&mut values, 100);
        assert!(values.len() <= 100);
        // First element should be preserved
        assert_eq!(values[0], 0.0);
    }

    #[test]
    fn test_collect_sampled_pixels_small_tile() {
        let pixels = create_uniform_image(32, 32, 0.5);
        let mut values = Vec::new();
        collect_sampled_pixels(&pixels, 0, 32, 0, 32, 32, &mut values);
        // Small tile should collect all or most pixels
        assert!(values.len() >= 100);
        assert!(values.iter().all(|&v| (v - 0.5).abs() < 0.01));
    }

    #[test]
    fn test_collect_sampled_pixels_large_tile() {
        let pixels = create_uniform_image(256, 256, 0.5);
        let mut values = Vec::new();
        collect_sampled_pixels(&pixels, 0, 256, 0, 256, 256, &mut values);
        // Large tile should sample ~MAX_TILE_SAMPLES
        assert!(values.len() <= MAX_TILE_SAMPLES * 2);
        assert!(values.iter().all(|&v| (v - 0.5).abs() < 0.01));
    }

    #[test]
    fn test_collect_unmasked_pixels_none_masked() {
        let pixels = create_uniform_image(64, 64, 0.5);
        let mask = BitBuffer2::new_filled(64, 64, false);
        let mut values = Vec::new();
        collect_unmasked_pixels(&pixels, &mask, 0, 64, 0, 64, 64, &mut values);
        assert_eq!(values.len(), 64 * 64);
    }

    #[test]
    fn test_collect_unmasked_pixels_all_masked() {
        let pixels = create_uniform_image(64, 64, 0.5);
        let mask = BitBuffer2::new_filled(64, 64, true);
        let mut values = Vec::new();
        collect_unmasked_pixels(&pixels, &mask, 0, 64, 0, 64, 64, &mut values);
        assert_eq!(values.len(), 0);
    }

    #[test]
    fn test_collect_unmasked_pixels_partial_mask() {
        let width = 64;
        let height = 64;
        let pixels = create_uniform_image(width, height, 0.5);

        // Mask every other pixel
        let mut mask = BitBuffer2::new_filled(width, height, false);
        for y in 0..height {
            for x in 0..width {
                if (x + y) % 2 == 0 {
                    mask.set_xy(x, y, true);
                }
            }
        }

        let mut values = Vec::new();
        collect_unmasked_pixels(&pixels, &mask, 0, 64, 0, 64, 64, &mut values);
        // Half pixels should be unmasked
        assert_eq!(values.len(), 64 * 64 / 2);
    }

    #[test]
    fn test_collect_unmasked_pixels_partial_tile() {
        let pixels = create_uniform_image(100, 100, 0.5);
        let mask = BitBuffer2::new_filled(100, 100, false);
        let mut values = Vec::new();
        // Collect from a sub-region not aligned to 64-bit boundaries
        collect_unmasked_pixels(&pixels, &mask, 10, 70, 20, 80, 100, &mut values);
        assert_eq!(values.len(), 60 * 60);
    }

    #[test]
    fn test_tile_with_outliers_sigma_clipped() {
        let width = 64;
        let height = 64;
        let mut data = vec![0.5; width * height];

        // Add some outliers
        for val in data.iter_mut().take(10) {
            *val = 10.0; // Bright outliers
        }

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 64);

        let stats = grid.get(0, 0);
        // Median should be close to 0.5 despite outliers
        assert!((stats.sky - 0.5).abs() < 0.1);
    }

    #[test]
    fn test_tile_stats_sigma_nonzero_for_varied_data() {
        let width = 64;
        let height = 64;
        // Create data with variation
        let data: Vec<f32> = (0..width * height)
            .map(|i| 0.5 + (i % 10) as f32 * 0.01)
            .collect();

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 64);

        let stats = grid.get(0, 0);
        assert!(stats.sigma > 0.0);
    }

    #[test]
    fn test_median_filter_corner_tiles() {
        // Test that corner tiles (with fewer neighbors) are handled correctly
        let pixels = create_uniform_image(128, 128, 0.5);
        let grid = make_grid(&pixels, 32);

        // Corner tiles should still have valid stats
        let corners = [(0, 0), (3, 0), (0, 3), (3, 3)];
        for (tx, ty) in corners {
            let stats = grid.get(tx, ty);
            assert!((stats.sky - 0.5).abs() < 0.01);
        }
    }

    #[test]
    fn test_negative_pixel_values() {
        let width = 64;
        let height = 64;
        let data = vec![-0.5; width * height];

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 32);

        let stats = grid.get(0, 0);
        assert!((stats.sky - (-0.5)).abs() < 0.01);
    }

    #[test]
    fn test_find_lower_tile_y_negative_pos() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = make_grid(&pixels, 32);

        // Negative position should return 0
        assert_eq!(grid.find_lower_tile_y(-10.0), 0);
    }

    // --- Algorithm correctness tests ---
    // These verify the statistical algorithms produce mathematically correct results

    #[test]
    fn test_median_computation_correctness() {
        // Create image where we know exact median
        // Tile with values 1,2,3,4,5,6,7,8,9 should have median=5
        let width = 3;
        let height = 3;
        let data: Vec<f32> = (1..=9).map(|x| x as f32).collect();

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 3);

        let stats = grid.get(0, 0);
        assert!(
            (stats.sky - 5.0).abs() < 0.1,
            "Median of 1-9 should be 5, got {}",
            stats.sky
        );
    }

    #[test]
    fn test_sigma_computation_correctness() {
        // For uniform data, sigma should be 0
        let pixels = create_uniform_image(64, 64, 100.0);
        let grid = make_grid(&pixels, 64);

        let stats = grid.get(0, 0);
        assert!(
            stats.sigma < 0.001,
            "Uniform data should have sigma ~0, got {}",
            stats.sigma
        );
    }

    #[test]
    fn test_mad_sigma_known_value() {
        // MAD-based sigma for known distribution
        // For values [0,1,2,3,4,5,6,7,8,9]:
        // - Approximate median (used for performance) = 5 (upper-middle for even length)
        // - Deviations from median: [5,4,3,2,1,0,1,2,3,4]
        // - MAD = approximate median of deviations = 3
        // - sigma = MAD * 1.4826 ≈ 4.4
        let width = 10;
        let height = 1;
        let data: Vec<f32> = (0..10).map(|x| x as f32).collect();

        let pixels = Buffer2::new(width, height, data);

        // Use large tile to get all pixels
        let grid = make_grid(&pixels, 10);
        let stats = grid.get(0, 0);

        // Approximate median for even-length array returns the upper-middle element (5), the
        // mean is 4.5, and |mean − median| = 0.5 < 0.3σ ≈ 1.33, so the Pearson mode fires:
        // sky = 2.5·5 − 1.5·4.5 = 5.75. (The 0.5 "skew" is the fast-median convention on a
        // 10-sample fixture; on real ≥1000-sample tiles the offset is negligible.)
        assert!(
            (stats.sky - 5.75).abs() < 0.1,
            "Pearson-mode sky should be 5.75, got {}",
            stats.sky
        );
        // Sigma should be ~4.4 (MAD=3 * 1.4826)
        assert!(
            (stats.sigma - 4.4).abs() < 0.5,
            "Sigma should be ~4.4, got {}",
            stats.sigma
        );
    }

    #[test]
    fn test_3sigma_clipping_rejects_outliers() {
        // Background of 100 with a few extreme outliers
        // 3-sigma clipping should reject values > median + 3*sigma
        let width = 100;
        let height = 100;
        let mut data = vec![100.0; width * height];

        // Add 1% extreme outliers (100 pixels with value 10000)
        for i in 0..100 {
            data[i * 100] = 10000.0;
        }

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 100);

        let stats = grid.get(0, 0);

        // After sigma clipping, median should still be ~100
        assert!(
            (stats.sky - 100.0).abs() < 5.0,
            "Median should be ~100 after clipping outliers, got {}",
            stats.sky
        );
    }

    #[test]
    fn test_median_filter_3x3_correctness() {
        // Create 5x5 grid of tiles where center tile has outlier value
        // After 3x3 median filter, center should match neighbors
        let width = 160; // 5 tiles of 32 pixels
        let height = 160;
        let mut data = vec![50.0; width * height];

        // Make center tile (tile 2,2) have value 200
        for y in 64..96 {
            for x in 64..96 {
                data[y * width + x] = 200.0;
            }
        }

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 32);

        // Center tile should be filtered to ~50 (median of 8x50 + 1x200 = 50)
        let center = grid.get(2, 2);
        assert!(
            (center.sky - 50.0).abs() < 10.0,
            "Center tile should be ~50 after median filter, got {}",
            center.sky
        );
    }

    #[test]
    fn test_background_gradient_preserved() {
        // Linear gradient from 0 to 100 across image
        // Tile statistics should reflect local background level
        let width = 256;
        let height = 64;
        let data: Vec<f32> = (0..height)
            .flat_map(|_| (0..width).map(|x| x as f32 / width as f32 * 100.0))
            .collect();

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 64);

        // Left tiles should have lower median than right tiles
        let left = grid.get(0, 0);
        let right = grid.get(3, 0);

        assert!(
            right.sky > left.sky + 30.0,
            "Right tile median {} should be > left {} + 30",
            right.sky,
            left.sky
        );
        assert!(
            left.sky < 30.0,
            "Left tile median {} should be < 30",
            left.sky
        );
        assert!(
            right.sky > 70.0,
            "Right tile median {} should be > 70",
            right.sky
        );
    }

    #[test]
    fn test_sparse_stars_rejected() {
        // Simulate astronomical image: mostly background (100) with sparse bright stars
        let width = 128;
        let height = 128;
        let mut data = vec![100.0; width * height];

        // Add 20 "stars" with brightness 500-1000 (random positions)
        let star_positions = [
            (10, 10),
            (50, 20),
            (100, 30),
            (30, 60),
            (80, 70),
            (120, 80),
            (15, 100),
            (60, 110),
            (90, 120),
            (110, 115),
            (25, 25),
            (75, 45),
            (45, 75),
            (95, 95),
            (5, 55),
            (55, 5),
            (105, 55),
            (55, 105),
            (35, 35),
            (85, 85),
        ];

        for (x, y) in star_positions {
            // Star with some spread
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let nx = (x + dx).clamp(0, 127) as usize;
                    let ny = (y + dy).clamp(0, 127) as usize;
                    data[ny * width + nx] = 500.0 + (dx.abs() + dy.abs()) as f32 * -100.0;
                }
            }
        }

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 64);

        // All tiles should have median close to background (100)
        for ty in 0..grid.tiles_y() {
            for tx in 0..grid.tiles_x() {
                let stats = grid.get(tx, ty);
                assert!(
                    (stats.sky - 100.0).abs() < 20.0,
                    "Tile ({},{}) median {} should be ~100 (background)",
                    tx,
                    ty,
                    stats.sky
                );
            }
        }
    }

    #[test]
    fn test_mask_excludes_sources_correctly() {
        // Background 50, sources at 200
        let width = 64;
        let height = 64;
        let mut data = vec![50.0; width * height];

        // Add bright source in top-left quadrant
        for y in 0..32 {
            for x in 0..32 {
                data[y * width + x] = 200.0;
            }
        }

        let pixels = Buffer2::new(width, height, data);

        // Mask the bright source
        let mut mask = BitBuffer2::new_filled(width, height, false);
        for y in 0..32 {
            for x in 0..32 {
                mask.set_xy(x, y, true);
            }
        }

        let grid = make_grid_with_mask(&pixels, 32, &mask);

        // Top-left tile (0,0): all pixels masked → falls back to all pixels (200.0)
        // Bottom-right tile (1,1): no masked pixels → uses background value
        let br = grid.get(1, 1);
        assert!(
            (br.sky - 50.0).abs() < 5.0,
            "Unmasked tile median {} should be ~50",
            br.sky
        );
    }

    // --- Natural cubic spline tests ---

    #[test]
    fn test_solve_d2_two_points_gives_zero() {
        // Natural spline with 2 points: d2 = 0 everywhere (linear)
        let values = [10.0, 20.0];
        let centers = [0.0, 1.0];
        let mut d2 = [999.0; 2];
        let mut scratch = [0.0; 1];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);
        assert_eq!(d2[0], 0.0);
        assert_eq!(d2[1], 0.0);
    }

    #[test]
    fn test_solve_d2_linear_data_gives_zero() {
        // Linear function f(x) = 2x + 1 at x = 0, 1, 2, 3
        // Second derivative of a linear function is 0 everywhere
        let values = [1.0, 3.0, 5.0, 7.0];
        let centers = [0.0, 1.0, 2.0, 3.0];
        let mut d2 = [0.0; 4];
        let mut scratch = [0.0; 2];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);

        for (i, &d) in d2.iter().enumerate() {
            assert!(
                d.abs() < 1e-5,
                "d2[{}] = {} should be 0 for linear data",
                i,
                d
            );
        }
    }

    #[test]
    fn test_solve_d2_quadratic_data() {
        // f(x) = x² at x = 0, 1, 2, 3 → f = [0, 1, 4, 9]
        // True second derivative = 2 everywhere
        // Natural spline with n=4 points, uniform h=1:
        //   Interior equations (i=1, i=2):
        //   d2[0]=0, d2[3]=0 (natural BC)
        //
        //   i=1: h0*d2[0] + 2*(h0+h1)*d2[1] + h1*d2[2] = 6*((f2-f1)/h1 - (f1-f0)/h0)
        //        0 + 4*d2[1] + 1*d2[2] = 6*((4-1)/1 - (1-0)/1) = 6*(3-1) = 12
        //
        //   i=2: h1*d2[1] + 2*(h1+h2)*d2[2] + h2*d2[3] = 6*((f3-f2)/h2 - (f2-f1)/h1)
        //        1*d2[1] + 4*d2[2] + 0 = 6*((9-4)/1 - (4-1)/1) = 6*(5-3) = 12
        //
        //   System: 4*d2[1] + d2[2] = 12
        //           d2[1] + 4*d2[2] = 12
        //   Solution: d2[1] = d2[2] = 12/5 = 2.4
        let values = [0.0, 1.0, 4.0, 9.0];
        let centers = [0.0, 1.0, 2.0, 3.0];
        let mut d2 = [0.0; 4];
        let mut scratch = [0.0; 2];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);

        assert!(d2[0].abs() < 1e-6, "d2[0] = {}, expected 0", d2[0]);
        assert!(d2[3].abs() < 1e-6, "d2[3] = {}, expected 0", d2[3]);
        assert!(
            (d2[1] - 2.4).abs() < 1e-5,
            "d2[1] = {}, expected 2.4",
            d2[1]
        );
        assert!(
            (d2[2] - 2.4).abs() < 1e-5,
            "d2[2] = {}, expected 2.4",
            d2[2]
        );
    }

    #[test]
    fn test_solve_d2_non_uniform_spacing() {
        // f(x) = x² at x = 0, 1, 3 → f = [0, 1, 9]
        // h0 = 1, h1 = 2
        // One interior equation (i=1):
        //   2*(h0+h1)*d2[1] = 6*((f2-f1)/h1 - (f1-f0)/h0)
        //   2*(1+2)*d2[1] = 6*((9-1)/2 - (1-0)/1) = 6*(4-1) = 18
        //   6*d2[1] = 18 → d2[1] = 3
        let values = [0.0, 1.0, 9.0];
        let centers = [0.0, 1.0, 3.0];
        let mut d2 = [0.0; 3];
        let mut scratch = [0.0; 1];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);

        assert!(d2[0].abs() < 1e-6, "d2[0] = {}, expected 0", d2[0]);
        assert!(d2[2].abs() < 1e-6, "d2[2] = {}, expected 0", d2[2]);
        assert!(
            (d2[1] - 3.0).abs() < 1e-5,
            "d2[1] = {}, expected 3.0",
            d2[1]
        );
    }

    #[test]
    fn test_cubic_spline_eval_endpoints() {
        // At t=0: should return f0; at t=1: should return f1
        let f0 = 10.0;
        let f1 = 20.0;
        let d0 = 5.0;
        let d1 = -3.0;
        let h = 32.0;

        let val_0 = cubic_spline_eval(f0, f1, d0, d1, h, 0.0);
        let val_1 = cubic_spline_eval(f0, f1, d0, d1, h, 1.0);

        assert!(
            (val_0 - f0).abs() < 1e-6,
            "t=0: expected {}, got {}",
            f0,
            val_0
        );
        assert!(
            (val_1 - f1).abs() < 1e-6,
            "t=1: expected {}, got {}",
            f1,
            val_1
        );
    }

    #[test]
    fn test_cubic_spline_eval_midpoint() {
        // At t=0.5, using f(t) = ct*f0 + t*f1 - t*ct*((2-t)*a + (1+t)*b):
        //   = (f0+f1)/2 - 0.375*(a+b)
        // where a = h²/6*d0, b = h²/6*d1
        let f0 = 100.0;
        let f1 = 200.0;
        let h = 6.0; // h²/6 = 6
        let d0 = 2.0; // a = 6*2 = 12
        let d1 = -1.0; // b = 6*(-1) = -6
        // Expected: (100+200)/2 - 0.375*(12 + (-6)) = 150 - 0.375*6 = 150 - 2.25 = 147.75
        let val = cubic_spline_eval(f0, f1, d0, d1, h, 0.5);
        assert!(
            (val - 147.75).abs() < 1e-4,
            "t=0.5: expected 147.75, got {}",
            val
        );
    }

    #[test]
    fn test_cubic_spline_eval_zero_d2_is_linear() {
        // With d0=d1=0, the spline should be exactly linear
        let f0 = 10.0;
        let f1 = 50.0;
        let h = 32.0;

        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let val = cubic_spline_eval(f0, f1, 0.0, 0.0, h, t);
            let expected = (1.0 - t) * f0 + t * f1;
            assert!(
                (val - expected).abs() < 1e-5,
                "t={}: expected {}, got {}",
                t,
                expected,
                val
            );
        }
    }

    #[test]
    fn test_y_spline_derivatives_uniform_data() {
        // Uniform image → all medians equal → d2y = 0 everywhere
        let pixels = create_uniform_image(128, 128, 0.5);
        let grid = make_grid(&pixels, 32);

        for ty in 0..grid.tiles_y() {
            for tx in 0..grid.tiles_x() {
                assert!(
                    grid.d2y_sky(tx, ty).abs() < 1e-6,
                    "d2y_sky({},{}) = {}, expected 0",
                    tx,
                    ty,
                    grid.d2y_sky(tx, ty)
                );
                assert!(
                    grid.d2y_sigma(tx, ty).abs() < 1e-6,
                    "d2y_sigma({},{}) = {}, expected 0",
                    tx,
                    ty,
                    grid.d2y_sigma(tx, ty)
                );
            }
        }
    }

    #[test]
    fn test_y_spline_derivatives_single_row() {
        // Single row of tiles → d2y = 0 (no Y interpolation)
        let pixels = create_uniform_image(128, 32, 0.5);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.tiles_y(), 1);
        for tx in 0..grid.tiles_x() {
            assert_eq!(grid.d2y_sky(tx, 0), 0.0);
            assert_eq!(grid.d2y_sigma(tx, 0), 0.0);
        }
    }

    #[test]
    fn test_y_spline_derivatives_two_rows() {
        // Two rows of tiles → natural spline gives d2 = 0 at both endpoints
        let width = 64;
        let height = 64;
        let data: Vec<f32> = (0..height)
            .flat_map(|y| std::iter::repeat_n(y as f32 / height as f32, width))
            .collect();
        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.tiles_y(), 2);
        for tx in 0..grid.tiles_x() {
            assert_eq!(grid.d2y_sky(tx, 0), 0.0);
            assert_eq!(grid.d2y_sky(tx, 1), 0.0);
        }
    }

    #[test]
    fn test_y_spline_derivatives_natural_bc() {
        // With >= 3 rows, boundary d2 values should be 0 (natural BC)
        let width = 64;
        let height = 128;
        let data: Vec<f32> = (0..height)
            .flat_map(|y| std::iter::repeat_n(y as f32 / height as f32, width))
            .collect();
        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.tiles_y(), 4);
        for tx in 0..grid.tiles_x() {
            assert!(
                grid.d2y_sky(tx, 0).abs() < 1e-6,
                "Natural BC: d2y[{},0] = {}",
                tx,
                grid.d2y_sky(tx, 0)
            );
            assert!(
                grid.d2y_sky(tx, 3).abs() < 1e-6,
                "Natural BC: d2y[{},3] = {}",
                tx,
                grid.d2y_sky(tx, 3)
            );
        }
    }

    // --- Solver: 5+ point, symmetry, and edge cases ---

    #[test]
    fn test_solve_d2_single_point() {
        let values = [42.0];
        let centers = [5.0];
        let mut d2 = [999.0; 1];
        let mut scratch = [0.0; 1];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);
        assert_eq!(d2[0], 0.0);
    }

    #[test]
    fn test_solve_d2_empty() {
        let values: [f32; 0] = [];
        let centers: [f32; 0] = [];
        let mut d2: [f32; 0] = [];
        let mut scratch: [f32; 0] = [];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);
        // No-op, just shouldn't panic
    }

    #[test]
    fn test_solve_d2_five_points_cubic() {
        // f(x) = x³ at x = 0, 1, 2, 3, 4 → f = [0, 1, 8, 27, 64]
        // True f''(x) = 6x, so f''(0)=0, f''(1)=6, f''(2)=12, f''(3)=18, f''(4)=24
        // Natural BC forces d2[0]=0, d2[4]=0, so the spline won't match true f''
        // at interior points. We solve the 3×3 tridiagonal system:
        //
        // h = 1 (uniform spacing)
        // Interior equations (i=1,2,3):
        //   i=1: 4*d2[1] + d2[2] = 6*((f2-f1) - (f1-f0)) = 6*(7 - 1) = 36
        //   i=2: d2[1] + 4*d2[2] + d2[3] = 6*((f3-f2) - (f2-f1)) = 6*(19 - 7) = 72
        //   i=3: d2[2] + 4*d2[3] = 6*((f4-f3) - (f3-f2)) = 6*(37 - 19) = 108
        //
        // Forward elimination:
        //   Row 1: d = 4, cp[0] = 1/4, d2[1] = 36/4 = 9
        //   Row 2: d = 4 - 1*(1/4) = 15/4, cp[1] = 1/(15/4) = 4/15
        //          d2[2] = (72 - 1*9)/(15/4) = 63/(15/4) = 63*4/15 = 252/15 = 16.8
        //   Row 3: d = 4 - 1*(4/15) = 56/15
        //          d2[3] = (108 - 1*16.8)/(56/15) = 91.2/(56/15) = 91.2*15/56 = 1368/56 = 24.4286...
        //
        // Back substitution:
        //   d2[3] = 1368/56 = 24.42857...
        //   d2[2] = 16.8 - (4/15)*24.42857... = 16.8 - 6.51429... = 10.28571...
        //   d2[1] = 9 - (1/4)*10.28571... = 9 - 2.57143... = 6.42857...
        //
        // Exact fractions: d2[1] = 45/7, d2[2] = 72/7, d2[3] = 171/7
        let values = [0.0, 1.0, 8.0, 27.0, 64.0];
        let centers = [0.0, 1.0, 2.0, 3.0, 4.0];
        let mut d2 = [0.0; 5];
        let mut scratch = [0.0; 3];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);

        assert!(d2[0].abs() < 1e-5, "d2[0] = {}, expected 0", d2[0]);
        assert!(d2[4].abs() < 1e-5, "d2[4] = {}, expected 0", d2[4]);
        let expected_1 = 45.0 / 7.0; // 6.42857...
        let expected_2 = 72.0 / 7.0; // 10.28571...
        let expected_3 = 171.0 / 7.0; // 24.42857...
        assert!(
            (d2[1] - expected_1).abs() < 1e-4,
            "d2[1] = {}, expected {}",
            d2[1],
            expected_1
        );
        assert!(
            (d2[2] - expected_2).abs() < 1e-4,
            "d2[2] = {}, expected {}",
            d2[2],
            expected_2
        );
        assert!(
            (d2[3] - expected_3).abs() < 1e-4,
            "d2[3] = {}, expected {}",
            d2[3],
            expected_3
        );
    }

    #[test]
    fn test_solve_d2_symmetric_data() {
        // f = [1, 4, 9, 4, 1] at x = [0, 1, 2, 3, 4] (symmetric around x=2)
        // Symmetry requires d2[1] == d2[3] and d2[2] is the center value.
        //
        // Interior equations (uniform h=1):
        //   i=1: 4*d2[1] + d2[2] = 6*((9-4) - (4-1)) = 6*(5-3) = 12
        //   i=2: d2[1] + 4*d2[2] + d2[3] = 6*((4-9) - (9-4)) = 6*(-5-5) = -60
        //   i=3: d2[2] + 4*d2[3] = 6*((1-4) - (4-9)) = 6*(-3+5) = 12
        //
        // By symmetry d2[1] = d2[3]. Let a = d2[1] = d2[3], b = d2[2]:
        //   4a + b = 12
        //   a + 4b + a = -60 → 2a + 4b = -60 → a + 2b = -30
        //   From first: b = 12 - 4a
        //   Substitute: a + 2(12-4a) = -30 → a + 24 - 8a = -30 → -7a = -54 → a = 54/7
        //   b = 12 - 4*54/7 = 12 - 216/7 = (84-216)/7 = -132/7
        let values = [1.0, 4.0, 9.0, 4.0, 1.0];
        let centers = [0.0, 1.0, 2.0, 3.0, 4.0];
        let mut d2 = [0.0; 5];
        let mut scratch = [0.0; 3];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);

        let expected_sym = 54.0 / 7.0; // d2[1] = d2[3]
        let expected_center = -132.0 / 7.0; // d2[2]

        assert!(d2[0].abs() < 1e-5, "d2[0] = {}, expected 0", d2[0]);
        assert!(d2[4].abs() < 1e-5, "d2[4] = {}, expected 0", d2[4]);
        assert!(
            (d2[1] - expected_sym).abs() < 1e-4,
            "d2[1] = {}, expected {} (54/7)",
            d2[1],
            expected_sym
        );
        assert!(
            (d2[3] - expected_sym).abs() < 1e-4,
            "d2[3] = {}, expected {} (54/7)",
            d2[3],
            expected_sym
        );
        assert!(
            (d2[1] - d2[3]).abs() < 1e-6,
            "Symmetry broken: d2[1]={} != d2[3]={}",
            d2[1],
            d2[3]
        );
        assert!(
            (d2[2] - expected_center).abs() < 1e-4,
            "d2[2] = {}, expected {} (-132/7)",
            d2[2],
            expected_center
        );
    }

    // --- cubic_spline_eval edge cases ---

    #[test]
    fn test_cubic_spline_eval_h_zero_returns_f0() {
        // When h=0 (degenerate interval), should return f0
        let val = cubic_spline_eval(42.0, 99.0, 5.0, -3.0, 0.0, 0.5);
        assert_eq!(val, 42.0);
    }

    #[test]
    fn test_cubic_spline_eval_h_negative_returns_f0() {
        let val = cubic_spline_eval(42.0, 99.0, 5.0, -3.0, -1.0, 0.5);
        assert_eq!(val, 42.0);
    }

    // --- Roundtrip: solver + evaluator reproduces node values ---

    #[test]
    fn test_spline_roundtrip_reproduces_nodes() {
        // Solve d2 for f = [0, 1, 8, 27, 64] (x³), then verify that evaluating
        // the spline at each node point exactly reproduces the function value.
        let values = [0.0f32, 1.0, 8.0, 27.0, 64.0];
        let centers = [0.0f32, 1.0, 2.0, 3.0, 4.0];
        let mut d2 = [0.0f32; 5];
        let mut scratch = [0.0f32; 3];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);

        // At each node i, evaluate spline from interval [i-1, i] at t=1
        // and from interval [i, i+1] at t=0. Both should give values[i].
        for i in 0..5 {
            // From left interval (t=1): interval [i-1, i]
            if i > 0 {
                let h = centers[i] - centers[i - 1];
                let val = cubic_spline_eval(values[i - 1], values[i], d2[i - 1], d2[i], h, 1.0);
                assert!(
                    (val - values[i]).abs() < 1e-4,
                    "Node {} from left: expected {}, got {}",
                    i,
                    values[i],
                    val
                );
            }
            // From right interval (t=0): interval [i, i+1]
            if i < 4 {
                let h = centers[i + 1] - centers[i];
                let val = cubic_spline_eval(values[i], values[i + 1], d2[i], d2[i + 1], h, 0.0);
                assert!(
                    (val - values[i]).abs() < 1e-4,
                    "Node {} from right: expected {}, got {}",
                    i,
                    values[i],
                    val
                );
            }
        }
    }

    #[test]
    fn test_spline_roundtrip_interior_continuity() {
        // At each interior node, the value from the left interval (t=1) should
        // match the value from the right interval (t=0). This tests C0 continuity.
        // Also test that the first derivative is continuous (C1).
        let values = [2.0f32, 5.0, 3.0, 8.0, 1.0, 6.0];
        let centers = [0.0f32, 1.0, 2.0, 3.0, 4.0, 5.0];
        let mut d2 = [0.0f32; 6];
        let mut scratch = [0.0f32; 4];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);

        for i in 1..5 {
            let h_left = centers[i] - centers[i - 1];
            let h_right = centers[i + 1] - centers[i];

            let val_left =
                cubic_spline_eval(values[i - 1], values[i], d2[i - 1], d2[i], h_left, 1.0);
            let val_right =
                cubic_spline_eval(values[i], values[i + 1], d2[i], d2[i + 1], h_right, 0.0);

            assert!(
                (val_left - val_right).abs() < 1e-4,
                "C0 break at node {}: left={}, right={}",
                i,
                val_left,
                val_right
            );

            // C1 check: numerical derivative from both sides should match
            let eps = 1e-4;
            let val_left_m = cubic_spline_eval(
                values[i - 1],
                values[i],
                d2[i - 1],
                d2[i],
                h_left,
                1.0 - eps,
            );
            let val_right_p =
                cubic_spline_eval(values[i], values[i + 1], d2[i], d2[i + 1], h_right, eps);
            // Derivative from left: (val_left - val_left_m) / (eps * h_left)
            // Derivative from right: (val_right_p - val_right) / (eps * h_right)
            let deriv_left = (val_left - val_left_m) / (eps * h_left);
            let deriv_right = (val_right_p - val_right) / (eps * h_right);
            assert!(
                (deriv_left - deriv_right).abs() < 0.1,
                "C1 break at node {}: deriv_left={}, deriv_right={}",
                i,
                deriv_left,
                deriv_right
            );
        }
    }

    // --- compute_y_spline_derivatives with nonzero d2 ---

    #[test]
    fn test_y_spline_derivatives_quadratic_gradient() {
        // Create image where each row of tiles has quadratic Y values:
        // f(y) = y² → tile medians should approximate y_center²
        // With 4 tile rows (uniform h=32), same as test_solve_d2_quadratic_data
        // but through the full pipeline.
        let width = 64;
        let height = 128; // 4 tile rows of 32
        let data: Vec<f32> = (0..height)
            .flat_map(|y| {
                let val = (y as f32 / height as f32).powi(2); // [0, 1)
                std::iter::repeat_n(val, width)
            })
            .collect();

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.tiles_y(), 4);

        // Natural BC: endpoints should be 0
        for tx in 0..grid.tiles_x() {
            assert!(
                grid.d2y_sky(tx, 0).abs() < 1e-5,
                "d2y[{},0] = {}, expected 0",
                tx,
                grid.d2y_sky(tx, 0)
            );
            assert!(
                grid.d2y_sky(tx, 3).abs() < 1e-5,
                "d2y[{},3] = {}, expected 0",
                tx,
                grid.d2y_sky(tx, 3)
            );
        }

        // Interior d2 should be nonzero (positive, since f''(y²) > 0)
        for tx in 0..grid.tiles_x() {
            assert!(
                grid.d2y_sky(tx, 1) > 1e-4,
                "d2y[{},1] = {}, expected positive",
                tx,
                grid.d2y_sky(tx, 1)
            );
            assert!(
                grid.d2y_sky(tx, 2) > 1e-4,
                "d2y[{},2] = {}, expected positive",
                tx,
                grid.d2y_sky(tx, 2)
            );
        }

        // All columns should have the same d2 values (uniform X data)
        if grid.tiles_x() >= 2 {
            for ty in 0..grid.tiles_y() {
                let d0 = grid.d2y_sky(0, ty);
                for tx in 1..grid.tiles_x() {
                    assert!(
                        (grid.d2y_sky(tx, ty) - d0).abs() < 1e-5,
                        "d2y[{},{}] = {} != d2y[0,{}] = {}",
                        tx,
                        ty,
                        grid.d2y_sky(tx, ty),
                        ty,
                        d0
                    );
                }
            }
        }
    }

    #[test]
    fn test_photutils_sextractor_comparison() {
        // Test case similar to photutils/SExtractor documentation examples
        // Background level 1000 with noise sigma ~10
        let width = 256;
        let height = 256;

        // Generate pseudo-random noise using deterministic pattern
        let data: Vec<f32> = (0..width * height)
            .map(|i| {
                let noise = ((i * 7919 + 104729) % 1000) as f32 / 100.0 - 5.0; // -5 to +5
                1000.0 + noise * 2.0 // background 1000, noise ~10
            })
            .collect();

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 64);

        // Check all tiles have reasonable background estimate
        for ty in 0..grid.tiles_y() {
            for tx in 0..grid.tiles_x() {
                let stats = grid.get(tx, ty);
                // Background should be ~1000 ± 5
                assert!(
                    (stats.sky - 1000.0).abs() < 10.0,
                    "Tile ({},{}) median {} should be ~1000",
                    tx,
                    ty,
                    stats.sky
                );
                // Sigma should be reasonable (not zero, not huge)
                assert!(
                    stats.sigma > 1.0 && stats.sigma < 30.0,
                    "Tile ({},{}) sigma {} should be reasonable",
                    tx,
                    ty,
                    stats.sigma
                );
            }
        }
    }
}
