//! Algorithm step implementations for Markesteijn 1-pass demosaicing.
//!
//! Each step is a separate function that operates on the shared working buffers.
//! Steps are called sequentially by the orchestrator in `markesteijn.rs`.

use rayon::prelude::*;

use super::XTransImage;
use super::hex_lookup::HexLookup;
use super::markesteijn::NDIR;

/// Wrapper to send raw pointers across rayon threads.
/// SAFETY: Caller must ensure no data races (each thread writes to unique indices).
/// Access inner value via `.get()` — never `.0` — so that Edition 2024 closures
/// capture `&UnsafeSendPtr` (which is Sync) rather than the inner pointer field.
#[derive(Clone, Copy)]
struct UnsafeSendPtr<T: Copy>(T);
unsafe impl<T: Copy> Send for UnsafeSendPtr<T> {}
unsafe impl<T: Copy> Sync for UnsafeSendPtr<T> {}

impl<T: Copy> UnsafeSendPtr<T> {
    fn get(&self) -> T {
        self.0
    }
}

/// Direction offsets for derivative computation.
/// Maps direction index to (dy, dx) offset for the spatial Laplacian.
/// Dir 0 = horizontal (0,1), Dir 1 = vertical (1,0),
/// Dir 2 = diagonal (1,1), Dir 3 = anti-diagonal (1,-1).
const DIR_OFFSETS: [(i32, i32); NDIR] = [(0, 1), (1, 0), (1, 1), (1, -1)];

// ───────────────────────────────────────────────────────────────
// Precomputed color neighbor lookup for interpolate_missing_color
// ───────────────────────────────────────────────────────────────

/// For each X-Trans position, which interpolation strategy to use for a missing color.
/// Precomputed from the 6×6 pattern to avoid per-pixel pattern lookups.
#[derive(Debug, Clone, Copy)]
enum ColorInterpStrategy {
    /// Opposing pair found: average color differences from both neighbors.
    /// (dy_a, dx_a, dy_b, dx_b) are the offsets to the two neighbors.
    Pair {
        dy_a: i32,
        dx_a: i32,
        dy_b: i32,
        dx_b: i32,
    },
    /// Single neighbor found (no opposing pair has both matching).
    /// (dy, dx) is the offset to the neighbor.
    Single { dy: i32, dx: i32 },
    /// No neighbor of this color exists at distance 1 (shouldn't happen in valid X-Trans).
    None,
}

/// Precomputed interpolation strategies for all 36 X-Trans positions × 2 target colors.
/// Indexed as [row%6][col%6][target_color_index] where target_color_index: 0=red, 1=blue.
#[derive(Debug)]
struct ColorInterpLookup {
    /// For each position, the best strategies for interpolating red and blue.
    /// [row%6][col%6][0=red, 1=blue] = array of up to 2 strategies (H pair, V pair)
    /// sorted by expected quality (pairs first, then singles).
    strategies: [[[InterpEntry; 2]; 6]; 6],
}

/// A precomputed interpolation entry for one position + one target color.
#[derive(Debug, Clone, Copy)]
struct InterpEntry {
    /// Primary strategy (best pair or single neighbor).
    primary: ColorInterpStrategy,
    /// Secondary strategy (second pair, if available, for gradient comparison).
    secondary: ColorInterpStrategy,
}

impl ColorInterpLookup {
    fn new(pattern: &super::XTransPattern) -> Self {
        let mut strategies = [[[InterpEntry {
            primary: ColorInterpStrategy::None,
            secondary: ColorInterpStrategy::None,
        }; 2]; 6]; 6];

        // Cardinal direction pairs: horizontal (0,1)/(0,-1) and vertical (1,0)/(-1,0)
        let pairs: [((i32, i32), (i32, i32)); 2] = [((0, 1), (0, -1)), ((1, 0), (-1, 0))];
        // Single directions for fallback
        let singles: [(i32, i32); 4] = [(0, 1), (0, -1), (1, 0), (-1, 0)];

        for (row, row_strategies) in strategies.iter_mut().enumerate() {
            for (col, col_strategies) in row_strategies.iter_mut().enumerate() {
                for (tc_idx, &target_color) in [0u8, 2u8].iter().enumerate() {
                    let mut found_pairs: Vec<ColorInterpStrategy> = Vec::new();
                    let mut found_singles: Vec<ColorInterpStrategy> = Vec::new();

                    // Check pairs first
                    for &((dy_a, dx_a), (dy_b, dx_b)) in &pairs {
                        // Use wrapping add with +6 to handle negative offsets in modular arithmetic
                        let nr_a = (row as i32 + dy_a + 6) as usize;
                        let nc_a = (col as i32 + dx_a + 6) as usize;
                        let nr_b = (row as i32 + dy_b + 6) as usize;
                        let nc_b = (col as i32 + dx_b + 6) as usize;

                        if pattern.color_at(nr_a, nc_a) == target_color
                            && pattern.color_at(nr_b, nc_b) == target_color
                        {
                            found_pairs.push(ColorInterpStrategy::Pair {
                                dy_a,
                                dx_a,
                                dy_b,
                                dx_b,
                            });
                        }
                    }

                    // Check singles as fallback
                    for &(dy, dx) in &singles {
                        let nr = (row as i32 + dy + 6) as usize;
                        let nc = (col as i32 + dx + 6) as usize;
                        if pattern.color_at(nr, nc) == target_color {
                            found_singles.push(ColorInterpStrategy::Single { dy, dx });
                        }
                    }

                    let entry = &mut col_strategies[tc_idx];
                    if found_pairs.len() >= 2 {
                        entry.primary = found_pairs[0];
                        entry.secondary = found_pairs[1];
                    } else if found_pairs.len() == 1 {
                        entry.primary = found_pairs[0];
                        if let Some(&s) = found_singles.first() {
                            entry.secondary = s;
                        }
                    } else if let Some(&s) = found_singles.first() {
                        entry.primary = s;
                        if found_singles.len() >= 2 {
                            entry.secondary = found_singles[1];
                        }
                    }
                }
            }
        }

        Self { strategies }
    }

    #[inline(always)]
    fn get(&self, row: usize, col: usize, target_color_idx: usize) -> &InterpEntry {
        &self.strategies[row % 6][col % 6][target_color_idx]
    }
}

// ───────────────────────────────────────────────────────────────
// Step 1: Green min/max
// ───────────────────────────────────────────────────────────────

/// Compute green min/max bounds at each non-green pixel.
///
/// For green pixels, gmin=gmax=raw_value.
/// For non-green pixels, scans the first 6 hex neighbors to find
/// the range of nearby green values. This constrains green interpolation.
pub(super) fn compute_green_minmax(
    xtrans: &XTransImage,
    hex: &HexLookup,
    gmin: &mut [f32],
    gmax: &mut [f32],
) {
    let width = xtrans.width;
    let height = xtrans.height;
    assert_eq!(gmin.len(), width * height);
    assert_eq!(gmax.len(), width * height);

    gmin.par_chunks_mut(width)
        .zip(gmax.par_chunks_mut(width))
        .enumerate()
        .for_each(|(y, (gmin_row, gmax_row))| {
            let raw_y = y + xtrans.top_margin;
            for x in 0..width {
                let raw_x = x + xtrans.left_margin;
                let color = xtrans.pattern.color_at(raw_y, raw_x);

                if color == 1 {
                    let val = xtrans.data[raw_y * xtrans.raw_width + raw_x];
                    gmin_row[x] = val;
                    gmax_row[x] = val;
                } else {
                    let hex_offsets = hex.get(raw_y, raw_x);
                    let mut min_g = f32::MAX;
                    let mut max_g = f32::MIN;

                    for ho in &hex_offsets[..6] {
                        let ny = raw_y as i32 + ho.dy;
                        let nx = raw_x as i32 + ho.dx;

                        if ny >= 0
                            && nx >= 0
                            && (ny as usize) < xtrans.raw_height
                            && (nx as usize) < xtrans.raw_width
                        {
                            let g = xtrans.data[ny as usize * xtrans.raw_width + nx as usize];
                            min_g = min_g.min(g);
                            max_g = max_g.max(g);
                        }
                    }

                    if min_g == f32::MAX {
                        min_g = 0.0;
                        max_g = 0.0;
                    }

                    gmin_row[x] = min_g;
                    gmax_row[x] = max_g;
                }
            }
        });
}

// ───────────────────────────────────────────────────────────────
// Step 2: Interpolate green in 4 directions
// ───────────────────────────────────────────────────────────────

/// Interpolate green channel in 4 directions using weighted hexagonal neighbors.
///
/// For each non-green pixel, computes 4 green estimates (one per direction)
/// using Markesteijn's weighted formulas, clamped to [gmin, gmax].
/// For green pixels, all 4 directions get the raw value.
///
/// The green_dir buffer is laid out as [dir * pixels + y * width + x].
pub(super) fn interpolate_green(
    xtrans: &XTransImage,
    hex: &HexLookup,
    gmin: &[f32],
    gmax: &[f32],
    green_dir: &mut [f32],
) {
    let width = xtrans.width;
    let height = xtrans.height;
    let pixels = width * height;
    let raw_width = xtrans.raw_width;

    // SAFETY: We split green_dir into 4 disjoint direction slices and extract raw pointers.
    // Each parallel row writes to [y*width..(y+1)*width] within each direction slice,
    // so no two threads write to the same index.
    let dir_send = {
        let (dir0, rest) = green_dir.split_at_mut(pixels);
        let (dir1, rest) = rest.split_at_mut(pixels);
        let (dir2, dir3) = rest.split_at_mut(pixels);
        UnsafeSendPtr([
            dir0.as_mut_ptr(),
            dir1.as_mut_ptr(),
            dir2.as_mut_ptr(),
            dir3.as_mut_ptr(),
        ])
    };

    (0..height).into_par_iter().for_each(|y| {
        let dir_ptrs = dir_send.get();
        let raw_y = y + xtrans.top_margin;
        let row_off = y * width;

        for x in 0..width {
            let raw_x = x + xtrans.left_margin;
            let color = xtrans.pattern.color_at(raw_y, raw_x);

            if color == 1 {
                let val = xtrans.data[raw_y * raw_width + raw_x];
                for ptr in &dir_ptrs {
                    // SAFETY: row_off + x is unique per (y, x), no data race
                    unsafe { *ptr.add(row_off + x) = val };
                }
            } else {
                let hex_offsets = hex.get(raw_y, raw_x);
                let raw_val = xtrans.data[raw_y * raw_width + raw_x];
                let lo = gmin[row_off + x];
                let hi = gmax[row_off + x];

                let read = |dy: i32, dx: i32| -> f32 {
                    let ny = raw_y as i32 + dy;
                    let nx = raw_x as i32 + dx;
                    if ny >= 0
                        && nx >= 0
                        && (ny as usize) < xtrans.raw_height
                        && (nx as usize) < raw_width
                    {
                        xtrans.data[ny as usize * raw_width + nx as usize]
                    } else {
                        raw_val
                    }
                };

                let h = hex_offsets;

                let n0 = read(h[0].dy, h[0].dx);
                let n1 = read(h[1].dy, h[1].dx);
                let n0_2 = read(2 * h[0].dy, 2 * h[0].dx);
                let n1_2 = read(2 * h[1].dy, 2 * h[1].dx);
                let color_a = 0.6796875 * (n0 + n1) - 0.1796875 * (n0_2 + n1_2);

                let n2 = read(h[2].dy, h[2].dx);
                let n3 = read(h[3].dy, h[3].dx);
                let same_color_neighbor = read(-h[2].dy, -h[2].dx);
                let color_b =
                    0.87109375 * n3 + 0.12890625 * n2 + 0.359375 * (raw_val - same_color_neighbor);

                let n4 = read(h[4].dy, h[4].dx);
                let n4_m2 = read(-2 * h[4].dy, -2 * h[4].dx);
                let n4_p3 = read(3 * h[4].dy, 3 * h[4].dx);
                let n4_m3 = read(-3 * h[4].dy, -3 * h[4].dx);
                let color_c0 =
                    0.640625 * n4 + 0.359375 * n4_m2 + 0.12890625 * (2.0 * raw_val - n4_p3 - n4_m3);

                let n5 = read(h[5].dy, h[5].dx);
                let n5_m2 = read(-2 * h[5].dy, -2 * h[5].dx);
                let n5_p3 = read(3 * h[5].dy, 3 * h[5].dx);
                let n5_m3 = read(-3 * h[5].dy, -3 * h[5].dx);
                let color_c1 =
                    0.640625 * n5 + 0.359375 * n5_m2 + 0.12890625 * (2.0 * raw_val - n5_p3 - n5_m3);

                let flip = if !raw_y.wrapping_sub(hex.sgrow).is_multiple_of(3) {
                    1
                } else {
                    0
                };

                let colors = [color_a, color_b, color_c0, color_c1];
                for (c, &val) in colors.iter().enumerate() {
                    let d = c ^ flip;
                    let clamped = val.clamp(lo, hi);
                    // SAFETY: row_off + x is unique per (y, x), no data race
                    unsafe { *dir_ptrs[d].add(row_off + x) = clamped };
                }
            }
        }
    });
}

// ───────────────────────────────────────────────────────────────
// Step 3+4: Fused R/B interpolation + YPbPr + derivatives
// ───────────────────────────────────────────────────────────────

/// Compute per-direction RGB + YPbPr derivatives.
///
/// Stores full RGB per direction in `rgb_dir` for reuse in blend step.
/// Computes YPbPr from RGB, then spatial Laplacian derivatives.
///
/// Both passes are row-parallel across all (direction, row) pairs.
pub(super) fn compute_rgb_and_derivatives(
    xtrans: &XTransImage,
    green_dir: &[f32],
    rgb_dir: &mut [f32],
    drv: &mut [f32],
) {
    let width = xtrans.width;
    let height = xtrans.height;
    let pixels = width * height;
    let row_stride = width * 3;

    // Precompute color neighbor lookup to avoid per-pixel pattern searches
    let color_lookup = ColorInterpLookup::new(&xtrans.pattern);

    // Pass 1: Compute per-direction RGB in parallel across all (dir, row) pairs.
    // Layout: rgb_dir[dir * pixels * 3 + y * width * 3 + x * 3 + {R,G,B}]
    rgb_dir
        .par_chunks_mut(row_stride)
        .enumerate()
        .for_each(|(flat_idx, rgb_row)| {
            let d = flat_idx / height;
            let y = flat_idx % height;
            compute_rgb_row(xtrans, green_dir, &color_lookup, d, pixels, y, rgb_row);
        });

    // Pass 2: Compute YPbPr derivatives — fully row-parallel across (dir, row).
    // YPbPr is computed on-the-fly from stored RGB, no separate buffer needed.
    drv.par_chunks_mut(width)
        .enumerate()
        .for_each(|(flat_idx, drv_row)| {
            let d = flat_idx / height;
            let y = flat_idx % height;
            let (dir_dy, dir_dx) = DIR_OFFSETS[d];
            let rgb_base = d * pixels * 3;

            for (x, drv_val) in drv_row.iter_mut().enumerate() {
                let (yc, pbc, prc) = rgb_to_ypbpr_at(rgb_dir, rgb_base, y, x, row_stride);

                let fy = (y as i32 + dir_dy) as usize;
                let fx = (x as i32 + dir_dx) as usize;
                let (yf, pbf, prf) = if fy < height && fx < width {
                    rgb_to_ypbpr_at(rgb_dir, rgb_base, fy, fx, row_stride)
                } else {
                    (yc, pbc, prc)
                };

                let by = y as i32 - dir_dy;
                let bx = x as i32 - dir_dx;
                let (yb, pbb, prb) =
                    if by >= 0 && bx >= 0 && (by as usize) < height && (bx as usize) < width {
                        rgb_to_ypbpr_at(rgb_dir, rgb_base, by as usize, bx as usize, row_stride)
                    } else {
                        (yc, pbc, prc)
                    };

                let dy = 2.0 * yc - yf - yb;
                let dpb = 2.0 * pbc - pbf - pbb;
                let dpr = 2.0 * prc - prf - prb;

                *drv_val = dy * dy + dpb * dpb + dpr * dpr;
            }
        });
}

/// Convert RGB at a given position to YPbPr.
#[inline(always)]
fn rgb_to_ypbpr_at(
    rgb_dir: &[f32],
    rgb_base: usize,
    y: usize,
    x: usize,
    row_stride: usize,
) -> (f32, f32, f32) {
    let off = rgb_base + y * row_stride + x * 3;
    let r = rgb_dir[off];
    let g = rgb_dir[off + 1];
    let b = rgb_dir[off + 2];
    let luma = 0.2627 * r + 0.6780 * g + 0.0593 * b;
    let pb = (b - luma) * 0.56433;
    let pr = (r - luma) * 0.67815;
    (luma, pb, pr)
}

/// Compute one row of per-direction RGB values.
fn compute_rgb_row(
    xtrans: &XTransImage,
    green_dir: &[f32],
    color_lookup: &ColorInterpLookup,
    dir: usize,
    pixels: usize,
    y: usize,
    out: &mut [f32], // length = width * 3
) {
    let width = xtrans.width;
    let raw_width = xtrans.raw_width;
    let green_base = dir * pixels;

    let interp = |x: usize, target_color_idx: usize| -> f32 {
        interpolate_missing_color_fast(
            xtrans,
            green_dir,
            color_lookup,
            green_base,
            y,
            x,
            target_color_idx,
        )
    };

    for x in 0..width {
        let raw_y = y + xtrans.top_margin;
        let raw_x = x + xtrans.left_margin;
        let color = xtrans.pattern.color_at(raw_y, raw_x);
        let raw_val = xtrans.data[raw_y * raw_width + raw_x];
        let green = green_dir[green_base + y * width + x];

        let (r, g, b) = match color {
            0 => (raw_val, green, interp(x, 1)),
            1 => (interp(x, 0), raw_val, interp(x, 1)),
            2 => (interp(x, 0), green, raw_val),
            _ => unreachable!(),
        };

        out[x * 3] = r;
        out[x * 3 + 1] = g;
        out[x * 3 + 2] = b;
    }
}

/// Interpolate a missing color using precomputed lookup table.
///
/// Uses green-guided color difference method with precomputed neighbor positions.
/// When two opposing pairs are available, picks the one with smaller green gradient.
#[inline]
fn interpolate_missing_color_fast(
    xtrans: &XTransImage,
    green_dir: &[f32],
    color_lookup: &ColorInterpLookup,
    green_base: usize,
    y: usize,
    x: usize,
    target_color_idx: usize, // 0=red, 1=blue
) -> f32 {
    let width = xtrans.width;
    let height = xtrans.height;
    let raw_width = xtrans.raw_width;
    let raw_y = y + xtrans.top_margin;
    let raw_x = x + xtrans.left_margin;
    let green_center = green_dir[green_base + y * width + x];

    let entry = color_lookup.get(raw_y, raw_x, target_color_idx);

    let eval = |strategy: &ColorInterpStrategy| -> Option<(f32, f32)> {
        match *strategy {
            ColorInterpStrategy::Pair {
                dy_a,
                dx_a,
                dy_b,
                dx_b,
            } => {
                let ay = raw_y as i32 + dy_a;
                let ax = raw_x as i32 + dx_a;
                let by = raw_y as i32 + dy_b;
                let bx = raw_x as i32 + dx_b;

                if ay < 0
                    || ax < 0
                    || by < 0
                    || bx < 0
                    || ay as usize >= xtrans.raw_height
                    || ax as usize >= raw_width
                    || by as usize >= xtrans.raw_height
                    || bx as usize >= raw_width
                {
                    return None;
                }

                let oy_a = (ay as usize).wrapping_sub(xtrans.top_margin);
                let ox_a = (ax as usize).wrapping_sub(xtrans.left_margin);
                let oy_b = (by as usize).wrapping_sub(xtrans.top_margin);
                let ox_b = (bx as usize).wrapping_sub(xtrans.left_margin);

                if oy_a >= height || ox_a >= width || oy_b >= height || ox_b >= width {
                    return None;
                }

                let ga = green_dir[green_base + oy_a * width + ox_a];
                let gb = green_dir[green_base + oy_b * width + ox_b];
                let grad = (green_center - ga).abs() + (green_center - gb).abs();

                let raw_a = xtrans.data[ay as usize * raw_width + ax as usize];
                let raw_b = xtrans.data[by as usize * raw_width + bx as usize];

                Some((green_center + 0.5 * ((raw_a - ga) + (raw_b - gb)), grad))
            }
            ColorInterpStrategy::Single { dy, dx } => {
                let ny = raw_y as i32 + dy;
                let nx = raw_x as i32 + dx;

                if ny < 0 || nx < 0 || ny as usize >= xtrans.raw_height || nx as usize >= raw_width
                {
                    return None;
                }

                let oy = (ny as usize).wrapping_sub(xtrans.top_margin);
                let ox = (nx as usize).wrapping_sub(xtrans.left_margin);
                if oy >= height || ox >= width {
                    return None;
                }

                let g_n = green_dir[green_base + oy * width + ox];
                let raw_n = xtrans.data[ny as usize * raw_width + nx as usize];
                Some((green_center + (raw_n - g_n), f32::MAX))
            }
            ColorInterpStrategy::None => None,
        }
    };

    let val = match (eval(&entry.primary), eval(&entry.secondary)) {
        (Some((vp, gp)), Some((vs, gs))) => {
            if gs < gp {
                vs
            } else {
                vp
            }
        }
        (Some((v, _)), None) | (None, Some((v, _))) => v,
        (None, None) => green_center,
    };

    val.max(0.0)
}

// ───────────────────────────────────────────────────────────────
// Step 5: Homogeneity maps
// ───────────────────────────────────────────────────────────────

/// Build homogeneity maps from per-direction derivatives.
///
/// Two sub-passes:
/// 1. Find minimum derivative across all 4 directions at each pixel → threshold = 8 × min
/// 2. In a 3×3 window, count how many pixels have drv ≤ threshold
pub(super) fn compute_homogeneity(drv: &[f32], width: usize, height: usize, homo: &mut [u8]) {
    let pixels = width * height;

    // Sub-pass 1: compute min derivative across directions and threshold
    // SAFETY: Every element is written by the parallel pass below before being read in sub-pass 2.
    let mut threshold = unsafe { crate::raw::alloc_uninit_vec::<f32>(pixels) };
    threshold
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, thr_row)| {
            for (x, thr) in thr_row.iter_mut().enumerate() {
                let idx = y * width + x;
                let mut min_drv = f32::MAX;
                for d in 0..NDIR {
                    min_drv = min_drv.min(drv[d * pixels + idx]);
                }
                *thr = min_drv * 8.0;
            }
        });

    // Sub-pass 2: count pixels in 3×3 window where drv ≤ threshold
    // Parallelize across all (direction, row) pairs for full core utilization.
    homo.fill(0);
    homo.par_chunks_mut(width)
        .enumerate()
        .for_each(|(flat_idx, homo_row)| {
            let d = flat_idx / height;
            let y = flat_idx % height;

            if y == 0 || y >= height - 1 {
                return; // Skip border rows
            }

            for (x, homo_val) in homo_row
                .iter_mut()
                .enumerate()
                .skip(1)
                .take(width.saturating_sub(2))
            {
                let mut count = 0u8;
                for vy in y - 1..=y + 1 {
                    for vx in x - 1..=x + 1 {
                        let nidx = vy * width + vx;
                        if drv[d * pixels + nidx] <= threshold[nidx] {
                            count += 1;
                        }
                    }
                }
                *homo_val = count;
            }
        });
}

// ───────────────────────────────────────────────────────────────
// Step 6: Final blend
// ───────────────────────────────────────────────────────────────

/// Final blending: sum homogeneity in 5×5 window, select best directions,
/// average pre-computed RGB from qualifying directions.
///
/// `output_buf` is a reusable buffer that will be truncated/resized to `pixels * 3`.
/// Pass a no-longer-needed buffer (e.g. `drv`) to avoid a fresh allocation.
pub(super) fn blend_final_from_rgb(
    rgb_dir: &[f32],
    homo: &[u8],
    width: usize,
    height: usize,
    mut output_buf: Vec<f32>,
) -> Vec<f32> {
    let pixels = width * height;
    let row_stride = width * 3;
    let needed = pixels * 3;

    // Reuse the provided buffer — truncate to needed size (no allocation).
    // Every element is overwritten by the parallel pass below.
    output_buf.truncate(needed);
    let mut rgb = output_buf;

    rgb.par_chunks_mut(row_stride)
        .enumerate()
        .for_each(|(y, rgb_row)| {
            for x in 0..width {
                // Sum homogeneity in 5×5 window for each direction
                let mut hm = [0u32; NDIR];
                for d in 0..NDIR {
                    for vy in y.saturating_sub(2)..=(y + 2).min(height - 1) {
                        for vx in x.saturating_sub(2)..=(x + 2).min(width - 1) {
                            hm[d] += homo[d * pixels + vy * width + vx] as u32;
                        }
                    }
                }

                // Find max homogeneity score
                let max_hm = *hm.iter().max().unwrap();
                let threshold = max_hm - (max_hm >> 3); // 7/8 of max

                // Average RGB from directions meeting threshold
                let mut avg_r = 0.0f32;
                let mut avg_g = 0.0f32;
                let mut avg_b = 0.0f32;
                let mut count = 0u32;

                for (d, &h) in hm.iter().enumerate() {
                    if h >= threshold {
                        let off = d * pixels * 3 + y * row_stride + x * 3;
                        avg_r += rgb_dir[off];
                        avg_g += rgb_dir[off + 1];
                        avg_b += rgb_dir[off + 2];
                        count += 1;
                    }
                }

                if count > 0 {
                    let inv = 1.0 / count as f32;
                    rgb_row[x * 3] = (avg_r * inv).max(0.0);
                    rgb_row[x * 3 + 1] = (avg_g * inv).max(0.0);
                    rgb_row[x * 3 + 2] = (avg_b * inv).max(0.0);
                }
            }
        });

    rgb
}

#[cfg(test)]
mod tests {
    use super::super::hex_lookup::HexLookup;
    use super::super::{XTransImage, XTransPattern};
    use super::*;

    fn test_pattern() -> XTransPattern {
        XTransPattern::new([
            [1, 0, 1, 1, 2, 1],
            [2, 1, 2, 0, 1, 0],
            [1, 2, 1, 1, 0, 1],
            [1, 2, 1, 1, 0, 1],
            [0, 1, 0, 2, 1, 2],
            [1, 0, 1, 1, 2, 1],
        ])
    }

    fn make_xtrans(
        data: &[f32],
        raw_w: usize,
        raw_h: usize,
        w: usize,
        h: usize,
        top: usize,
        left: usize,
    ) -> XTransImage<'_> {
        XTransImage::with_margins(data, raw_w, raw_h, w, h, top, left, test_pattern())
    }

    #[test]
    fn test_green_minmax_uniform() {
        let raw_w = 24;
        let raw_h = 24;
        let w = 12;
        let h = 12;
        let data = vec![0.5f32; raw_w * raw_h];
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);
        let hex = HexLookup::new(&xtrans.pattern);

        let mut gmin = vec![0.0f32; w * h];
        let mut gmax = vec![0.0f32; w * h];
        compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);

        // Uniform 0.5 input → gmin=gmax=0.5 everywhere
        for i in 0..w * h {
            assert!((gmin[i] - 0.5).abs() < 1e-6, "gmin[{}] = {}", i, gmin[i]);
            assert!((gmax[i] - 0.5).abs() < 1e-6, "gmax[{}] = {}", i, gmax[i]);
        }
    }

    #[test]
    fn test_green_minmax_bounds() {
        let raw_w = 24;
        let raw_h = 24;
        let w = 12;
        let h = 12;
        // Create gradient data
        let data: Vec<f32> = (0..raw_w * raw_h)
            .map(|i| (i as f32) / (raw_w * raw_h) as f32)
            .collect();
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);
        let hex = HexLookup::new(&xtrans.pattern);

        let mut gmin = vec![0.0f32; w * h];
        let mut gmax = vec![0.0f32; w * h];
        compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);

        // gmin should always be <= gmax
        for i in 0..w * h {
            assert!(
                gmin[i] <= gmax[i],
                "gmin[{}] = {} > gmax = {}",
                i,
                gmin[i],
                gmax[i]
            );
        }
    }

    #[test]
    fn test_interpolate_green_uniform() {
        let raw_w = 24;
        let raw_h = 24;
        let w = 12;
        let h = 12;
        let data = vec![0.5f32; raw_w * raw_h];
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);
        let hex = HexLookup::new(&xtrans.pattern);

        let mut gmin = vec![0.0f32; w * h];
        let mut gmax = vec![0.0f32; w * h];
        compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);

        let mut green_dir = vec![0.0f32; NDIR * w * h];
        interpolate_green(&xtrans, &hex, &gmin, &gmax, &mut green_dir);

        // All green values should be 0.5 for uniform input
        for d in 0..NDIR {
            for i in 0..w * h {
                let g = green_dir[d * w * h + i];
                assert!(
                    (g - 0.5).abs() < 0.05,
                    "green_dir[{}][{}] = {} (expected ~0.5)",
                    d,
                    i,
                    g
                );
            }
        }
    }

    #[test]
    fn test_homogeneity_uniform_derivatives() {
        let w = 12;
        let h = 12;
        let pixels = w * h;

        // Uniform derivatives → all directions equally good
        let drv = vec![1.0f32; NDIR * pixels];
        let mut homo = vec![0u8; NDIR * pixels];
        compute_homogeneity(&drv, w, h, &mut homo);

        // Interior pixels should have equal homogeneity across all directions
        for y in 2..h - 2 {
            for x in 2..w - 2 {
                let idx = y * w + x;
                let h0 = homo[idx];
                let h1 = homo[pixels + idx];
                let h2 = homo[2 * pixels + idx];
                let h3 = homo[3 * pixels + idx];
                assert_eq!(h0, h1, "Homogeneity mismatch at ({},{})", y, x);
                assert_eq!(h1, h2, "Homogeneity mismatch at ({},{})", y, x);
                assert_eq!(h2, h3, "Homogeneity mismatch at ({},{})", y, x);
                // With uniform drv=1.0, threshold = 8.0, all drv <= threshold
                // so count should be 9 (full 3×3 window)
                assert_eq!(h0, 9, "Expected 9 at ({},{}), got {}", y, x, h0);
            }
        }
    }

    #[test]
    fn test_ypbpr_conversion_white() {
        // White (1,1,1) → Y=1, Pb=0, Pr=0
        let y: f32 = 0.2627 * 1.0 + 0.6780 * 1.0 + 0.0593 * 1.0;
        let pb: f32 = (1.0 - y) * 0.56433;
        let pr: f32 = (1.0 - y) * 0.67815;
        assert!((y - 1.0).abs() < 1e-4, "Y={}", y);
        assert!(pb.abs() < 1e-4, "Pb={}", pb);
        assert!(pr.abs() < 1e-4, "Pr={}", pr);
    }

    #[test]
    fn test_ypbpr_conversion_black() {
        // Black (0,0,0) → Y=0, Pb=0, Pr=0
        let y: f32 = 0.2627 * 0.0 + 0.6780 * 0.0 + 0.0593 * 0.0;
        let pb: f32 = (0.0 - y) * 0.56433;
        let pr: f32 = (0.0 - y) * 0.67815;
        assert_eq!(y, 0.0);
        assert_eq!(pb, 0.0);
        assert_eq!(pr, 0.0);
    }
}
