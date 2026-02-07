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
                    // At most 2 pairs and 4 singles possible — use fixed arrays
                    let mut found_pairs = [ColorInterpStrategy::None; 2];
                    let mut n_pairs = 0usize;
                    let mut found_singles = [ColorInterpStrategy::None; 4];
                    let mut n_singles = 0usize;

                    // Check pairs first
                    for &((dy_a, dx_a), (dy_b, dx_b)) in &pairs {
                        // +6 handles negative offsets in modular arithmetic
                        let nr_a = (row as i32 + dy_a + 6) as usize;
                        let nc_a = (col as i32 + dx_a + 6) as usize;
                        let nr_b = (row as i32 + dy_b + 6) as usize;
                        let nc_b = (col as i32 + dx_b + 6) as usize;

                        if pattern.color_at(nr_a, nc_a) == target_color
                            && pattern.color_at(nr_b, nc_b) == target_color
                        {
                            found_pairs[n_pairs] = ColorInterpStrategy::Pair {
                                dy_a,
                                dx_a,
                                dy_b,
                                dx_b,
                            };
                            n_pairs += 1;
                        }
                    }

                    // Check singles as fallback
                    for &(dy, dx) in &singles {
                        let nr = (row as i32 + dy + 6) as usize;
                        let nc = (col as i32 + dx + 6) as usize;
                        if pattern.color_at(nr, nc) == target_color {
                            found_singles[n_singles] = ColorInterpStrategy::Single { dy, dx };
                            n_singles += 1;
                        }
                    }

                    let entry = &mut col_strategies[tc_idx];
                    if n_pairs >= 2 {
                        entry.primary = found_pairs[0];
                        entry.secondary = found_pairs[1];
                    } else if n_pairs == 1 {
                        entry.primary = found_pairs[0];
                        if n_singles > 0 {
                            entry.secondary = found_singles[0];
                        }
                    } else if n_singles > 0 {
                        entry.primary = found_singles[0];
                        if n_singles >= 2 {
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
// Step 3: Derivatives (RGB recomputed on-the-fly)
// ───────────────────────────────────────────────────────────────

/// Compute YPbPr spatial derivatives by recomputing RGB on-the-fly from green_dir.
///
/// For each direction, computes a Laplacian in that direction's offset,
/// storing the squared derivative magnitude per pixel. RGB is computed
/// per-pixel using `compute_rgb_pixel` instead of reading from a materialized buffer.
pub(super) fn compute_derivatives(xtrans: &XTransImage, green_dir: &[f32], drv: &mut [f32]) {
    let width = xtrans.width;
    let height = xtrans.height;
    let pixels = width * height;
    let color_lookup = ColorInterpLookup::new(&xtrans.pattern);

    drv.par_chunks_mut(width)
        .enumerate()
        .for_each(|(flat_idx, drv_row)| {
            let d = flat_idx / height;
            let y = flat_idx % height;
            let (dir_dy, dir_dx) = DIR_OFFSETS[d];
            let green_base = d * pixels;
            let y_interior = y >= 1 && y + 1 < height;

            for (x, drv_val) in drv_row.iter_mut().enumerate() {
                let interior = y_interior && x >= 1 && x + 1 < width;
                let (rc, gc, bc) =
                    compute_rgb_pixel(xtrans, green_dir, &color_lookup, green_base, y, x, interior);
                let (yc, pbc, prc) = rgb_to_ypbpr(rc, gc, bc);

                let fy = (y as i32 + dir_dy) as usize;
                let fx = (x as i32 + dir_dx) as usize;
                let (yf, pbf, prf) = if fy < height && fx < width {
                    let fi = fy >= 1 && fy + 1 < height && fx >= 1 && fx + 1 < width;
                    let (rf, gf, bf) =
                        compute_rgb_pixel(xtrans, green_dir, &color_lookup, green_base, fy, fx, fi);
                    rgb_to_ypbpr(rf, gf, bf)
                } else {
                    (yc, pbc, prc)
                };

                let by = y as i32 - dir_dy;
                let bx = x as i32 - dir_dx;
                let (yb, pbb, prb) =
                    if by >= 0 && bx >= 0 && (by as usize) < height && (bx as usize) < width {
                        let byu = by as usize;
                        let bxu = bx as usize;
                        let bi = byu >= 1 && byu + 1 < height && bxu >= 1 && bxu + 1 < width;
                        let (rb, gb, bb) = compute_rgb_pixel(
                            xtrans,
                            green_dir,
                            &color_lookup,
                            green_base,
                            byu,
                            bxu,
                            bi,
                        );
                        rgb_to_ypbpr(rb, gb, bb)
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

/// Convert RGB to YPbPr.
#[inline(always)]
fn rgb_to_ypbpr(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let luma = 0.2627 * r + 0.6780 * g + 0.0593 * b;
    let pb = (b - luma) * 0.56433;
    let pr = (r - luma) * 0.67815;
    (luma, pb, pr)
}

/// Compute RGB for a single pixel given its direction's green estimate.
///
/// Returns (R, G, B) by filling in missing channels using green-guided
/// color difference interpolation. Used by `compute_derivatives` and
/// `blend_final` to recompute RGB on-the-fly without materializing rgb_dir.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn compute_rgb_pixel(
    xtrans: &XTransImage,
    green_dir: &[f32],
    color_lookup: &ColorInterpLookup,
    green_base: usize,
    y: usize,
    x: usize,
    interior: bool,
) -> (f32, f32, f32) {
    let width = xtrans.width;
    let raw_width = xtrans.raw_width;
    let raw_y = y + xtrans.top_margin;
    let raw_x = x + xtrans.left_margin;
    let color = xtrans.pattern.color_at(raw_y, raw_x);
    let raw_val = xtrans.data[raw_y * raw_width + raw_x];
    let green = green_dir[green_base + y * width + x];

    let interp = |target_color_idx: usize| -> f32 {
        interpolate_missing_color_fast(
            xtrans,
            green_dir,
            color_lookup,
            green_base,
            y,
            x,
            target_color_idx,
            interior,
        )
    };

    match color {
        0 => (raw_val, green, interp(1)),
        1 => (interp(0), raw_val, interp(1)),
        2 => (interp(0), green, raw_val),
        _ => unreachable!(),
    }
}

/// Interpolate a missing color using precomputed lookup table.
///
/// Uses green-guided color difference method with precomputed neighbor positions.
/// When two opposing pairs are available, picks the one with smaller green gradient.
///
/// `interior` hint: when true, skips all bounds checks (caller guarantees all neighbors
/// are within bounds). This eliminates the main overhead for ~99% of pixels.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn interpolate_missing_color_fast(
    xtrans: &XTransImage,
    green_dir: &[f32],
    color_lookup: &ColorInterpLookup,
    green_base: usize,
    y: usize,
    x: usize,
    target_color_idx: usize, // 0=red, 1=blue
    interior: bool,
) -> f32 {
    let width = xtrans.width;
    let raw_width = xtrans.raw_width;
    let raw_y = y + xtrans.top_margin;
    let raw_x = x + xtrans.left_margin;
    let green_center = green_dir[green_base + y * width + x];

    let entry = color_lookup.get(raw_y, raw_x, target_color_idx);

    if interior {
        // Fast path: no bounds checks needed. All neighbors at distance ±1 are valid
        // because we're at least 1 pixel from any edge in both raw and active coordinates.
        let eval_unchecked = |strategy: &ColorInterpStrategy| -> Option<(f32, f32)> {
            match *strategy {
                ColorInterpStrategy::Pair {
                    dy_a,
                    dx_a,
                    dy_b,
                    dx_b,
                } => {
                    let oy_a = (y as i32 + dy_a) as usize;
                    let ox_a = (x as i32 + dx_a) as usize;
                    let oy_b = (y as i32 + dy_b) as usize;
                    let ox_b = (x as i32 + dx_b) as usize;

                    let ga = green_dir[green_base + oy_a * width + ox_a];
                    let gb = green_dir[green_base + oy_b * width + ox_b];
                    let grad = (green_center - ga).abs() + (green_center - gb).abs();

                    let raw_a = xtrans.data[(raw_y as i32 + dy_a) as usize * raw_width
                        + (raw_x as i32 + dx_a) as usize];
                    let raw_b = xtrans.data[(raw_y as i32 + dy_b) as usize * raw_width
                        + (raw_x as i32 + dx_b) as usize];

                    Some((green_center + 0.5 * ((raw_a - ga) + (raw_b - gb)), grad))
                }
                ColorInterpStrategy::Single { dy, dx } => {
                    let oy = (y as i32 + dy) as usize;
                    let ox = (x as i32 + dx) as usize;

                    let g_n = green_dir[green_base + oy * width + ox];
                    let raw_n = xtrans.data
                        [(raw_y as i32 + dy) as usize * raw_width + (raw_x as i32 + dx) as usize];
                    Some((green_center + (raw_n - g_n), f32::MAX))
                }
                ColorInterpStrategy::None => None,
            }
        };

        let val = match (
            eval_unchecked(&entry.primary),
            eval_unchecked(&entry.secondary),
        ) {
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
        return val.max(0.0);
    }

    // Slow path: full bounds checking for border pixels.
    let height = xtrans.height;

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
// Step 4: Homogeneity maps
// ───────────────────────────────────────────────────────────────

/// Build homogeneity maps from per-direction derivatives.
///
/// Two sub-passes:
/// 1. Find minimum derivative across all 4 directions at each pixel → threshold = 8 × min
/// 2. In a 3×3 window, count how many pixels have drv ≤ threshold
pub(super) fn compute_homogeneity(
    drv: &[f32],
    width: usize,
    height: usize,
    homo: &mut [u8],
    threshold: &mut [f32],
) {
    let pixels = width * height;

    // Sub-pass 1: compute min derivative across directions and threshold
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
    // Every element is written (border pixels get 0), avoiding a costly fill(0) of the full buffer.
    homo.par_chunks_mut(width)
        .enumerate()
        .for_each(|(flat_idx, homo_row)| {
            let d = flat_idx / height;
            let y = flat_idx % height;

            if y == 0 || y >= height - 1 {
                homo_row.fill(0);
                return;
            }

            // First and last columns are border pixels
            homo_row[0] = 0;
            if width > 1 {
                homo_row[width - 1] = 0;
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
// Step 5: Final blend
// ───────────────────────────────────────────────────────────────

/// Build a summed area table (SAT) from a u8 slice.
///
/// For an input `data` of size `height × width`, produces `(height+1) × (width+1)` u32 values
/// where `sat[(y+1)*(width+1) + (x+1)]` = sum of data[0..=y][0..=x].
/// Row 0 and column 0 of the SAT are zero (sentinel row/col for boundary handling).
fn build_summed_area_table(data: &[u8], width: usize, height: usize) -> Vec<u32> {
    let sat_w = width + 1;
    let sat_h = height + 1;
    // SAFETY: Every element is written below — sentinel row/col are zeroed explicitly,
    // and interior elements are computed from the row above + running row sum.
    let mut sat = unsafe { crate::raw::alloc_uninit_vec::<u32>(sat_w * sat_h) };

    // Zero the sentinel row (y=0)
    sat[..sat_w].fill(0);

    for y in 0..height {
        // Zero the sentinel column (x=0)
        sat[(y + 1) * sat_w] = 0;
        let mut row_sum = 0u32;
        for x in 0..width {
            row_sum += data[y * width + x] as u32;
            sat[(y + 1) * sat_w + (x + 1)] = row_sum + sat[y * sat_w + (x + 1)];
        }
    }

    sat
}

/// Query a rectangular sum from a summed area table.
/// Computes sum of data[y0..=y1][x0..=x1] in O(1).
#[inline(always)]
fn sat_query(sat: &[u32], sat_w: usize, y0: usize, x0: usize, y1: usize, x1: usize) -> u32 {
    // SAT is offset by +1 in both dimensions, so (y1+1, x1+1) maps to bottom-right inclusive.
    sat[(y1 + 1) * sat_w + (x1 + 1)] + sat[y0 * sat_w + x0]
        - sat[y0 * sat_w + (x1 + 1)]
        - sat[(y1 + 1) * sat_w + x0]
}

/// Final blending: sum homogeneity in 5×5 window, select best directions,
/// recompute RGB on-the-fly for qualifying directions and average.
///
/// Uses summed area tables for O(1) per-pixel window queries instead of O(25).
/// RGB is recomputed per winning direction using `compute_rgb_pixel` rather than
/// reading from a materialized 12P buffer, trading ~10-20 FLOPs per winning
/// direction for a ~1.1 GB memory reduction.
///
/// `output` is a preallocated slice of length `pixels * 3` where the final RGB is written.
pub(super) fn blend_final(
    xtrans: &XTransImage,
    green_dir: &[f32],
    homo: &[u8],
    width: usize,
    height: usize,
    output: &mut [f32],
) {
    let pixels = width * height;
    let row_stride = width * 3;
    assert_eq!(output.len(), pixels * 3);

    let color_lookup = ColorInterpLookup::new(&xtrans.pattern);

    // Build summed area tables for each direction's homogeneity map.
    let sats: Vec<Vec<u32>> = (0..NDIR)
        .map(|d| build_summed_area_table(&homo[d * pixels..(d + 1) * pixels], width, height))
        .collect();
    let sat_w = width + 1;

    output
        .par_chunks_mut(row_stride)
        .enumerate()
        .for_each(|(y, rgb_row)| {
            let y0 = y.saturating_sub(2);
            let y1 = (y + 2).min(height - 1);
            let y_interior = y >= 1 && y + 1 < height;

            for x in 0..width {
                let x0 = x.saturating_sub(2);
                let x1 = (x + 2).min(width - 1);

                // Query 5×5 homogeneity sum for each direction via SAT
                let mut hm = [0u32; NDIR];
                for d in 0..NDIR {
                    hm[d] = sat_query(&sats[d], sat_w, y0, x0, y1, x1);
                }

                // Find max homogeneity score
                let max_hm = *hm.iter().max().unwrap();
                let threshold = max_hm - (max_hm >> 3); // 7/8 of max

                // Average RGB from qualifying directions, recomputing on-the-fly
                let mut avg_r = 0.0f32;
                let mut avg_g = 0.0f32;
                let mut avg_b = 0.0f32;
                let mut count = 0u32;
                let interior = y_interior && x >= 1 && x + 1 < width;

                for (d, &h) in hm.iter().enumerate() {
                    if h >= threshold {
                        let green_base = d * pixels;
                        let (r, g, b) = compute_rgb_pixel(
                            xtrans,
                            green_dir,
                            &color_lookup,
                            green_base,
                            y,
                            x,
                            interior,
                        );
                        avg_r += r;
                        avg_g += g;
                        avg_b += b;
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
        let mut threshold = vec![0.0f32; pixels];
        compute_homogeneity(&drv, w, h, &mut homo, &mut threshold);

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

    // ── SAT (Summed Area Table) tests ────────────────────────────

    #[test]
    fn test_sat_uniform_ones() {
        // 4×3 grid of all 1s
        let data = vec![1u8; 4 * 3];
        let sat = build_summed_area_table(&data, 4, 3);

        // Full image sum = 12
        assert_eq!(sat_query(&sat, 5, 0, 0, 2, 3), 12);
        // Single pixel (0,0) = 1
        assert_eq!(sat_query(&sat, 5, 0, 0, 0, 0), 1);
        // First row sum = 4
        assert_eq!(sat_query(&sat, 5, 0, 0, 0, 3), 4);
        // First column sum = 3
        assert_eq!(sat_query(&sat, 5, 0, 0, 2, 0), 3);
        // 2×2 top-left corner = 4
        assert_eq!(sat_query(&sat, 5, 0, 0, 1, 1), 4);
        // 2×2 bottom-right corner = 4
        assert_eq!(sat_query(&sat, 5, 1, 2, 2, 3), 4);
    }

    #[test]
    fn test_sat_sequential_values() {
        // 3×3 grid: [1,2,3; 4,5,6; 7,8,9]
        let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9];
        let sat = build_summed_area_table(&data, 3, 3);

        // Full sum = 45
        assert_eq!(sat_query(&sat, 4, 0, 0, 2, 2), 45);
        // Center pixel only = 5
        assert_eq!(sat_query(&sat, 4, 1, 1, 1, 1), 5);
        // Middle row = 4+5+6 = 15
        assert_eq!(sat_query(&sat, 4, 1, 0, 1, 2), 15);
        // Bottom-right 2×2 = 5+6+8+9 = 28
        assert_eq!(sat_query(&sat, 4, 1, 1, 2, 2), 28);
    }

    #[test]
    fn test_sat_single_pixel() {
        let data = vec![42u8];
        let sat = build_summed_area_table(&data, 1, 1);
        assert_eq!(sat_query(&sat, 2, 0, 0, 0, 0), 42);
    }

    #[test]
    fn test_sat_single_row() {
        let data = vec![1, 2, 3, 4, 5];
        let sat = build_summed_area_table(&data, 5, 1);
        // Full row = 15
        assert_eq!(sat_query(&sat, 6, 0, 0, 0, 4), 15);
        // Middle 3 elements = 2+3+4 = 9
        assert_eq!(sat_query(&sat, 6, 0, 1, 0, 3), 9);
    }

    #[test]
    fn test_sat_single_column() {
        let data = vec![1, 2, 3, 4, 5];
        let sat = build_summed_area_table(&data, 1, 5);
        // Full column = 15
        assert_eq!(sat_query(&sat, 2, 0, 0, 4, 0), 15);
        // Middle 3 = 2+3+4 = 9
        assert_eq!(sat_query(&sat, 2, 1, 0, 3, 0), 9);
    }

    #[test]
    fn test_sat_zeros() {
        let data = vec![0u8; 4 * 4];
        let sat = build_summed_area_table(&data, 4, 4);
        assert_eq!(sat_query(&sat, 5, 0, 0, 3, 3), 0);
    }

    // ── Homogeneity border tests ─────────────────────────────────

    #[test]
    fn test_homogeneity_border_pixels_are_zero() {
        let w = 12;
        let h = 12;
        let pixels = w * h;

        let drv = vec![1.0f32; NDIR * pixels];
        let mut homo = vec![0xFFu8; NDIR * pixels]; // fill with garbage to detect missing writes
        let mut threshold = vec![0.0f32; pixels];
        compute_homogeneity(&drv, w, h, &mut homo, &mut threshold);

        for d in 0..NDIR {
            // Top and bottom rows should be 0
            for x in 0..w {
                assert_eq!(homo[d * pixels + x], 0, "dir={d} top row x={x}");
                assert_eq!(
                    homo[d * pixels + (h - 1) * w + x],
                    0,
                    "dir={d} bottom row x={x}"
                );
            }
            // First and last columns should be 0
            for y in 0..h {
                assert_eq!(homo[d * pixels + y * w], 0, "dir={d} left col y={y}");
                assert_eq!(
                    homo[d * pixels + y * w + (w - 1)],
                    0,
                    "dir={d} right col y={y}"
                );
            }
        }
    }

    #[test]
    fn test_homogeneity_one_dominant_direction() {
        let w = 12;
        let h = 12;
        let pixels = w * h;

        // Direction 0 has very low derivatives, others have high
        let mut drv = vec![100.0f32; NDIR * pixels];
        drv[..pixels].fill(0.1); // dir 0
        let mut homo = vec![0u8; NDIR * pixels];
        let mut threshold = vec![0.0f32; pixels];
        compute_homogeneity(&drv, w, h, &mut homo, &mut threshold);

        // Interior pixels: dir 0 should have high homogeneity, others low
        for y in 2..h - 2 {
            for x in 2..w - 2 {
                let idx = y * w + x;
                let h0 = homo[idx];
                let h1 = homo[pixels + idx];
                assert!(
                    h0 > h1,
                    "At ({y},{x}): dir0 homo={h0} should be > dir1 homo={h1}"
                );
            }
        }
    }

    // ── ColorInterpLookup tests ──────────────────────────────────

    #[test]
    fn test_color_interp_lookup_coverage() {
        let pattern = test_pattern();
        let lookup = ColorInterpLookup::new(&pattern);

        let mut has_pair = 0;
        let mut has_single = 0;
        let mut has_none = 0;

        // Verify all 36×2 entries are valid (Pair, Single, or None) and count coverage
        for row in 0..6 {
            for col in 0..6 {
                for tc_idx in 0..2 {
                    let entry = lookup.get(row, col, tc_idx);
                    match entry.primary {
                        ColorInterpStrategy::Pair { .. } => has_pair += 1,
                        ColorInterpStrategy::Single { .. } => has_single += 1,
                        ColorInterpStrategy::None => has_none += 1,
                    }
                }
            }
        }

        // X-Trans has good color coverage — most positions should have pairs or singles.
        // Some positions may have None (no neighbor of that color at distance ±1).
        assert!(
            has_pair + has_single > has_none,
            "Too many None entries: pair={has_pair} single={has_single} none={has_none}"
        );
        // At least some pairs should exist
        assert!(has_pair > 0, "No pair strategies found");
    }

    #[test]
    fn test_color_interp_lookup_pair_symmetry() {
        let pattern = test_pattern();
        let lookup = ColorInterpLookup::new(&pattern);

        // For Pair strategies, the two neighbors should be in opposing directions
        for row in 0..6 {
            for col in 0..6 {
                for tc_idx in 0..2 {
                    let entry = lookup.get(row, col, tc_idx);
                    if let ColorInterpStrategy::Pair {
                        dy_a,
                        dx_a,
                        dy_b,
                        dx_b,
                    } = entry.primary
                    {
                        // Opposing means dy_a = -dy_b and dx_a = -dx_b
                        assert_eq!(
                            dy_a, -dy_b,
                            "Pair at ({row},{col},tc={tc_idx}) dy not opposing"
                        );
                        assert_eq!(
                            dx_a, -dx_b,
                            "Pair at ({row},{col},tc={tc_idx}) dx not opposing"
                        );
                    }
                }
            }
        }
    }

    // ── Interpolate missing color: interior vs border consistency ─

    #[test]
    fn test_interpolate_interior_matches_border_path() {
        // For interior pixels, the fast (unchecked) and slow (checked) paths
        // should produce identical results.
        let raw_w = 24;
        let raw_h = 24;
        let w = 12;
        let h = 12;
        // Gradient data to exercise actual interpolation
        let data: Vec<f32> = (0..raw_w * raw_h)
            .map(|i| (i as f32) / (raw_w * raw_h) as f32)
            .collect();
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);
        let color_lookup = ColorInterpLookup::new(&xtrans.pattern);

        let pixels = w * h;
        let mut green_dir = vec![0.0f32; NDIR * pixels];
        let hex = HexLookup::new(&xtrans.pattern);
        let mut gmin = vec![0.0f32; pixels];
        let mut gmax = vec![1.0f32; pixels];
        compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);
        interpolate_green(&xtrans, &hex, &gmin, &gmax, &mut green_dir);

        // Test interior pixels (y in 1..h-1, x in 1..w-1) with both paths
        for d in 0..NDIR {
            let green_base = d * pixels;
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    for tc_idx in 0..2 {
                        let fast = interpolate_missing_color_fast(
                            &xtrans,
                            &green_dir,
                            &color_lookup,
                            green_base,
                            y,
                            x,
                            tc_idx,
                            true, // interior
                        );
                        let slow = interpolate_missing_color_fast(
                            &xtrans,
                            &green_dir,
                            &color_lookup,
                            green_base,
                            y,
                            x,
                            tc_idx,
                            false, // border path
                        );
                        assert!(
                            (fast - slow).abs() < 1e-6,
                            "Mismatch at d={d} y={y} x={x} tc={tc_idx}: fast={fast} slow={slow}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_interpolate_border_pixels_dont_panic() {
        // Border pixels with bounds checking should not panic
        let raw_w = 14;
        let raw_h = 14;
        let w = 6;
        let h = 6;
        let data: Vec<f32> = (0..raw_w * raw_h)
            .map(|i| (i as f32) / (raw_w * raw_h) as f32)
            .collect();
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 4, 4);
        let color_lookup = ColorInterpLookup::new(&xtrans.pattern);

        let pixels = w * h;
        let green_dir = vec![0.5f32; NDIR * pixels];

        // Test all edge pixels with border (slow) path
        for d in 0..NDIR {
            let green_base = d * pixels;
            for y in 0..h {
                for x in 0..w {
                    for tc_idx in 0..2 {
                        let val = interpolate_missing_color_fast(
                            &xtrans,
                            &green_dir,
                            &color_lookup,
                            green_base,
                            y,
                            x,
                            tc_idx,
                            false,
                        );
                        assert!(val.is_finite(), "NaN at d={d} y={y} x={x} tc={tc_idx}");
                        assert!(val >= 0.0, "Negative at d={d} y={y} x={x} tc={tc_idx}");
                    }
                }
            }
        }
    }

    // ── Blend final tests ────────────────────────────────────────

    #[test]
    fn test_blend_uniform_homo_produces_uniform_output() {
        // With uniform input and uniform homogeneity, output should be uniform
        let raw_w = 24;
        let raw_h = 24;
        let w = 12;
        let h = 12;
        let pixels = w * h;
        let data = vec![0.5f32; raw_w * raw_h];
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);
        let hex = HexLookup::new(&xtrans.pattern);

        let mut gmin = vec![0.0f32; pixels];
        let mut gmax = vec![1.0f32; pixels];
        compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);

        let mut green_dir = vec![0.0f32; NDIR * pixels];
        interpolate_green(&xtrans, &hex, &gmin, &gmax, &mut green_dir);

        // Uniform homogeneity → all directions qualify
        let homo = vec![9u8; NDIR * pixels];

        let mut output = vec![0.0f32; pixels * 3];
        blend_final(&xtrans, &green_dir, &homo, w, h, &mut output);

        // Uniform 0.5 input → output should be approximately 0.5 for all channels
        for (i, &v) in output.iter().enumerate() {
            assert!((v - 0.5).abs() < 0.05, "Pixel {i}: expected ~0.5, got {v}");
        }
    }

    #[test]
    fn test_blend_one_dominant_direction() {
        // With one dominant direction, output should match that direction's RGB
        let raw_w = 30;
        let raw_h = 30;
        let w = 18;
        let h = 18;
        let pixels = w * h;
        let data = vec![0.5f32; raw_w * raw_h];
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);
        let hex = HexLookup::new(&xtrans.pattern);

        let mut gmin = vec![0.0f32; pixels];
        let mut gmax = vec![1.0f32; pixels];
        compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);

        let mut green_dir = vec![0.0f32; NDIR * pixels];
        interpolate_green(&xtrans, &hex, &gmin, &gmax, &mut green_dir);

        // Dir 0 has highest homogeneity (9), others have 0
        let mut homo = vec![0u8; NDIR * pixels];
        homo[..pixels].fill(9);

        let mut output_one = vec![0.0f32; pixels * 3];
        blend_final(&xtrans, &green_dir, &homo, w, h, &mut output_one);

        // All directions equally good
        let homo_all = vec![9u8; NDIR * pixels];
        let mut output_all = vec![0.0f32; pixels * 3];
        blend_final(&xtrans, &green_dir, &homo_all, w, h, &mut output_all);

        // With uniform input, both should give similar results
        for i in 0..output_one.len() {
            assert!(
                (output_one[i] - output_all[i]).abs() < 0.05,
                "Pixel {i}: one_dir={} all_dir={}",
                output_one[i],
                output_all[i]
            );
        }

        // Output should have no NaN or negative values
        for (i, &v) in output_one.iter().enumerate() {
            assert!(v.is_finite(), "NaN at {i}");
            assert!(v >= 0.0, "Negative at {i}");
        }
    }
}
