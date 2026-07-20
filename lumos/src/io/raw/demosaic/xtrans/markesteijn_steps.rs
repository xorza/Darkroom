//! Algorithm step implementations for Markesteijn 1-pass demosaicing.
//!
//! Each step is a separate function that operates on the shared working buffers.
//! Steps are called sequentially by the orchestrator in `markesteijn.rs`.

use rayon::prelude::*;

use crate::io::raw::demosaic::xtrans::XTransImage;
use crate::io::raw::demosaic::xtrans::hex_lookup::HexLookup;
use crate::io::raw::demosaic::xtrans::markesteijn::NDIR;

use crate::concurrency::UnsafeSendPtr;

/// Direction offsets for derivative computation.
/// Maps direction index to (dy, dx) offset for the spatial Laplacian.
/// Dir 0 = horizontal (0,1), Dir 1 = vertical (1,0),
/// Dir 2 = diagonal (1,1), Dir 3 = anti-diagonal (1,-1).
const DIR_OFFSETS: [(i32, i32); NDIR] = [(0, 1), (1, 0), (1, 1), (1, -1)];
const GREEN_BLOCK_DIRECTIONS: usize = 2;

const MARK_INFO_BORDER: usize = 8;

/// Compute green min/max bounds at each non-green pixel.
///
/// For green pixels, gmin=gmax=raw_value.
/// For non-green pixels, scans the first 6 hex neighbors to find
/// the range of nearby green values. This constrains green interpolation.
pub(crate) fn compute_green_minmax(
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
                let color = xtrans.raw_pattern.color_at(raw_y, raw_x);

                if color == 1 {
                    let val = xtrans.read_normalized(raw_y, raw_x);
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
                            let g = xtrans.read_normalized(ny as usize, nx as usize);
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

/// Interpolate green channel in 4 directions using weighted hexagonal neighbors.
///
/// For each non-green pixel, computes 4 green estimates (one per direction)
/// using Markesteijn's weighted formulas, clamped to [gmin, gmax].
/// For green pixels, all 4 directions get the raw value.
///
/// The green_dir buffer is laid out as [dir * pixels + y * width + x].
pub(crate) fn interpolate_green(
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
        UnsafeSendPtr::new([
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
        // librtprocess stores the alternating-row candidates in the opposite direction slots.
        let flip = ((raw_y as i64 - hex.sgrow as i64).rem_euclid(3) == 0) as usize;

        for x in 0..width {
            let raw_x = x + xtrans.left_margin;
            let color = xtrans.raw_pattern.color_at(raw_y, raw_x);

            if color == 1 {
                let val = xtrans.read_normalized(raw_y, raw_x);
                for ptr in &dir_ptrs {
                    // SAFETY: row_off + x is unique per (y, x), no data race
                    unsafe { *ptr.add(row_off + x) = val };
                }
            } else {
                let hex_offsets = hex.get(raw_y, raw_x);
                let raw_val = xtrans.read_normalized(raw_y, raw_x);
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
                        xtrans.read_normalized(ny as usize, nx as usize)
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

#[inline(always)]
fn rb_index(color: u8) -> usize {
    debug_assert!(color == 0 || color == 2);
    (color / 2) as usize
}

#[inline(always)]
fn is_solitary_green(hex: &HexLookup, raw_y: usize, raw_x: usize) -> bool {
    raw_y % 3 == hex.sgrow && raw_x % 3 == hex.sgcol
}

#[inline(always)]
fn active_raw(xtrans: &XTransImage, y: usize, x: usize) -> f32 {
    xtrans.read_normalized(y + xtrans.top_margin, x + xtrans.left_margin)
}

#[inline(always)]
fn green_at(green_dir: &[f32], green_base: usize, width: usize, y: usize, x: usize) -> f32 {
    green_dir[green_base + y * width + x]
}

#[derive(Debug)]
struct SolitaryGreenCandidate {
    colors: [f32; 2],
    difference: f32,
}

#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn solitary_green_candidate(
    xtrans: &XTransImage,
    green_dir: &[f32],
    green_base: usize,
    y: usize,
    x: usize,
    candidate: usize,
) -> SolitaryGreenCandidate {
    let width = xtrans.width;
    let (dy, dx) = if candidate & 1 == 0 {
        (0isize, 1isize)
    } else {
        (1, 0)
    };
    let center_green = green_at(green_dir, green_base, width, y, x);
    let raw_y = y + xtrans.top_margin;
    let raw_x = x + xtrans.left_margin;
    let mut colors = [0.0; 2];
    let mut difference = 0.0;
    let mut target = xtrans.raw_pattern.color_at(raw_y, raw_x + 1);
    if candidate & 1 != 0 {
        target ^= 2;
    }

    for distance in 1..=2 {
        let oy = dy * distance;
        let ox = dx * distance;
        let plus_y = y.wrapping_add_signed(oy);
        let plus_x = x.wrapping_add_signed(ox);
        let minus_y = y.wrapping_add_signed(-oy);
        let minus_x = x.wrapping_add_signed(-ox);
        debug_assert_ne!(target, 1);

        let green_plus = green_at(green_dir, green_base, width, plus_y, plus_x);
        let green_minus = green_at(green_dir, green_base, width, minus_y, minus_x);
        let plus_native = xtrans
            .raw_pattern
            .color_at(raw_y.wrapping_add_signed(oy), raw_x.wrapping_add_signed(ox));
        let minus_native = xtrans.raw_pattern.color_at(
            raw_y.wrapping_add_signed(-oy),
            raw_x.wrapping_add_signed(-ox),
        );
        let raw_plus = if plus_native == target {
            active_raw(xtrans, plus_y, plus_x)
        } else {
            0.0
        };
        let raw_minus = if minus_native == target {
            active_raw(xtrans, minus_y, minus_x)
        } else {
            0.0
        };
        let green_correction = 2.0 * center_green - green_plus - green_minus;

        colors[rb_index(target)] = 0.5 * (green_correction + raw_plus + raw_minus);
        difference +=
            (green_plus - green_minus - raw_plus + raw_minus).powi(2) + green_correction.powi(2);
        target ^= 2;
    }

    SolitaryGreenCandidate { colors, difference }
}

#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn solitary_green_colors(
    xtrans: &XTransImage,
    green_dir: &[f32],
    green_base: usize,
    y: usize,
    x: usize,
    direction: usize,
) -> [f32; 2] {
    match direction {
        0 | 1 => solitary_green_candidate(xtrans, green_dir, green_base, y, x, direction).colors,
        2 | 3 => {
            let first = direction * 2 - 2;
            let horizontal = solitary_green_candidate(xtrans, green_dir, green_base, y, x, first);
            let vertical = solitary_green_candidate(xtrans, green_dir, green_base, y, x, first + 1);
            if horizontal.difference < vertical.difference {
                horizontal.colors
            } else {
                vertical.colors
            }
        }
        _ => unreachable!(),
    }
}

#[inline(always)]
fn color_before_opposite(
    colors: *mut [f32; 2],
    pixels: usize,
    direction: usize,
    width: usize,
    y: usize,
    x: usize,
    target: u8,
) -> f32 {
    // SAFETY: Initialization and the solitary-green stage are complete before this stage.
    unsafe { (*colors.add(direction * pixels + y * width + x))[rb_index(target)] }
}

#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn opposite_color(
    xtrans: &XTransImage,
    hex: &HexLookup,
    green_dir: &[f32],
    colors: *mut [f32; 2],
    pixels: usize,
    green_base: usize,
    y: usize,
    x: usize,
    direction: usize,
    target: u8,
) -> f32 {
    let width = xtrans.width;
    let raw_y = y + xtrans.top_margin;
    let center_green = green_at(green_dir, green_base, width, y, x);
    let primary_vertical = (raw_y as i64 - hex.sgrow as i64).rem_euclid(3) != 0;
    let (primary_dy, primary_dx) = if primary_vertical {
        (1isize, 0isize)
    } else {
        (0, 1)
    };
    let (long_dy, long_dx) = if primary_vertical {
        (0isize, 3isize)
    } else {
        (3, 0)
    };

    let gradient = |dy: isize, dx: isize| {
        let plus_y = y.wrapping_add_signed(dy);
        let plus_x = x.wrapping_add_signed(dx);
        let minus_y = y.wrapping_add_signed(-dy);
        let minus_x = x.wrapping_add_signed(-dx);
        (center_green - green_at(green_dir, green_base, width, plus_y, plus_x)).abs()
            + (center_green - green_at(green_dir, green_base, width, minus_y, minus_x)).abs()
    };

    let primary_gradient = gradient(primary_dy, primary_dx);
    let long_gradient = gradient(long_dy, long_dx);
    let primary_parity = usize::from(!primary_vertical);
    let use_primary = direction > 1
        || ((direction ^ primary_parity) & 1) != 0
        || primary_gradient < 2.0 * long_gradient;
    let (dy, dx) = if use_primary {
        (primary_dy, primary_dx)
    } else {
        (long_dy, long_dx)
    };
    let plus_y = y.wrapping_add_signed(dy);
    let plus_x = x.wrapping_add_signed(dx);
    let minus_y = y.wrapping_add_signed(-dy);
    let minus_x = x.wrapping_add_signed(-dx);
    let color_plus =
        color_before_opposite(colors, pixels, direction, width, plus_y, plus_x, target);
    let color_minus =
        color_before_opposite(colors, pixels, direction, width, minus_y, minus_x, target);
    let green_plus = green_at(green_dir, green_base, width, plus_y, plus_x);
    let green_minus = green_at(green_dir, green_base, width, minus_y, minus_x);

    center_green + 0.5 * (color_plus + color_minus - green_plus - green_minus)
}

#[inline(always)]
fn color_before_green_block(
    xtrans: &XTransImage,
    hex: &HexLookup,
    colors: *mut [f32; 2],
    direction: usize,
    y: usize,
    x: usize,
    target: u8,
) -> f32 {
    let native = xtrans
        .raw_pattern
        .color_at(y + xtrans.top_margin, x + xtrans.left_margin);
    debug_assert!(
        native != 1 || is_solitary_green(hex, y + xtrans.top_margin, x + xtrans.left_margin)
    );
    if native == target {
        active_raw(xtrans, y, x)
    } else {
        let pixels = xtrans.width * xtrans.height;
        // SAFETY: The solitary-green and opposite-color stages are complete before this stage.
        unsafe { (*colors.add(direction * pixels + y * xtrans.width + x))[rb_index(target)] }
    }
}

#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn green_block_colors(
    xtrans: &XTransImage,
    hex: &HexLookup,
    green_dir: &[f32],
    colors: *mut [f32; 2],
    green_base: usize,
    y: usize,
    x: usize,
    direction: usize,
) -> [f32; 2] {
    debug_assert!(direction < GREEN_BLOCK_DIRECTIONS);

    let width = xtrans.width;
    let offsets = hex.get(y + xtrans.top_margin, x + xtrans.left_margin);
    let first = offsets[direction * 2];
    let second = offsets[direction * 2 + 1];
    let first_y = y.wrapping_add_signed(first.dy as isize);
    let first_x = x.wrapping_add_signed(first.dx as isize);
    let second_y = y.wrapping_add_signed(second.dy as isize);
    let second_x = x.wrapping_add_signed(second.dx as isize);
    let center_green = green_at(green_dir, green_base, width, y, x);
    let first_green = green_at(green_dir, green_base, width, first_y, first_x);
    let second_green = green_at(green_dir, green_base, width, second_y, second_x);
    let asymmetric = first.dy + second.dy != 0 || first.dx + second.dx != 0;
    let (first_weight, divisor, green_correction) = if asymmetric {
        (
            2.0,
            3.0,
            3.0 * center_green - 2.0 * first_green - second_green,
        )
    } else {
        (1.0, 2.0, 2.0 * center_green - first_green - second_green)
    };
    let mut result = [0.0; 2];

    for target in [0, 2] {
        let first_color =
            color_before_green_block(xtrans, hex, colors, direction, first_y, first_x, target);
        let second_color =
            color_before_green_block(xtrans, hex, colors, direction, second_y, second_x, target);
        result[rb_index(target)] =
            (green_correction + first_weight * first_color + second_color) / divisor;
    }

    result
}

/// Reconstruct directional red/blue candidates in Markesteijn's three geometry stages.
pub(crate) fn reconstruct_colors(
    xtrans: &XTransImage,
    hex: &HexLookup,
    green_dir: &[f32],
    colors: &mut [[f32; 2]],
) {
    let width = xtrans.width;
    let height = xtrans.height;
    let pixels = width * height;
    assert_eq!(green_dir.len(), NDIR * pixels);
    assert_eq!(colors.len(), NDIR * pixels);

    colors
        .par_iter_mut()
        .enumerate()
        .for_each(|(flat, output)| {
            let direction = flat / pixels;
            let index = flat % pixels;
            let y = index / width;
            let x = index % width;
            let raw_y = y + xtrans.top_margin;
            let raw_x = x + xtrans.left_margin;
            let native = xtrans.raw_pattern.color_at(raw_y, raw_x);
            *output = [0.0; 2];
            if native != 1 {
                output[rb_index(native)] = active_raw(xtrans, y, x);
            } else if is_solitary_green(hex, raw_y, raw_x)
                && y >= 2
                && y + 2 < height
                && x >= 2
                && x + 2 < width
            {
                *output =
                    solitary_green_colors(xtrans, green_dir, direction * pixels, y, x, direction);
            }
        });

    let color_ptr = UnsafeSendPtr::new(colors.as_mut_ptr());
    (0..NDIR * pixels).into_par_iter().for_each(|flat| {
        let direction = flat / pixels;
        let index = flat % pixels;
        let y = index / width;
        let x = index % width;
        if y < 3 || y + 3 >= height || x < 3 || x + 3 >= width {
            return;
        }
        let raw_y = y + xtrans.top_margin;
        let raw_x = x + xtrans.left_margin;
        let native = xtrans.raw_pattern.color_at(raw_y, raw_x);
        if native == 1 {
            return;
        }
        let target = 2 - native;
        let value = opposite_color(
            xtrans,
            hex,
            green_dir,
            color_ptr.get(),
            pixels,
            direction * pixels,
            y,
            x,
            direction,
            target,
        );
        // SAFETY: Each task writes one unique colored site after reading only native or
        // solitary-green sites initialized by the preceding stage.
        unsafe {
            (*color_ptr.get().add(flat))[rb_index(target)] = value;
        }
    });

    (0..GREEN_BLOCK_DIRECTIONS * pixels)
        .into_par_iter()
        .for_each(|flat| {
            let direction = flat / pixels;
            let index = flat % pixels;
            let y = index / width;
            let x = index % width;
            if y < 2 || y + 2 >= height || x < 2 || x + 2 >= width {
                return;
            }
            let raw_y = y + xtrans.top_margin;
            let raw_x = x + xtrans.left_margin;
            if xtrans.raw_pattern.color_at(raw_y, raw_x) != 1
                || is_solitary_green(hex, raw_y, raw_x)
            {
                return;
            }
            let value = green_block_colors(
                xtrans,
                hex,
                green_dir,
                color_ptr.get(),
                direction * pixels,
                y,
                x,
                direction,
            );
            // SAFETY: Each task writes one unique 2×2-green site after reading only sites
            // completed by the preceding two stages.
            unsafe {
                *color_ptr.get().add(flat) = value;
            }
        });
}

/// Compute YPbPr spatial derivatives from the materialized directional candidates.
///
/// For each direction, computes a Laplacian in that direction's offset,
/// storing the squared derivative magnitude per pixel.
pub(crate) fn compute_derivatives(
    xtrans: &XTransImage,
    green_dir: &[f32],
    colors: &[[f32; 2]],
    drv: &mut [f32],
) {
    let width = xtrans.width;
    let height = xtrans.height;
    let pixels = width * height;
    let drv_ptr = UnsafeSendPtr::new(drv.as_mut_ptr());

    // Split each direction's rows into chunks for parallelism, and within each
    // chunk process rows sequentially with a sliding 3-row YPbPr cache.
    // This gives both multi-core utilization AND 3x reduction in
    // RGB row loads (each row computed once, reused as neighbor).
    // At chunk boundaries, one row is recomputed (negligible overhead).
    let chunk_size = 64;
    let chunks_per_dir = height.div_ceil(chunk_size);
    let total_chunks = NDIR * chunks_per_dir;

    (0..total_chunks).into_par_iter().for_each_init(
        || {
            // Allocate 3 row buffers once per rayon thread, reused across chunks.
            [
                vec![(0.0f32, 0.0f32, 0.0f32); width],
                vec![(0.0f32, 0.0f32, 0.0f32); width],
                vec![(0.0f32, 0.0f32, 0.0f32); width],
            ]
        },
        |rows, chunk_idx| {
            let d = chunk_idx / chunks_per_dir;
            let chunk_i = chunk_idx % chunks_per_dir;
            let y_start = chunk_i * chunk_size;
            let y_end = (y_start + chunk_size).min(height);

            let (dir_dy, dir_dx) = DIR_OFFSETS[d];
            let green_base = d * pixels;
            let drv_base = d * pixels;

            if dir_dy == 0 {
                // Direction 0: horizontal (0, +1). Forward/backward are in the same row.
                // Reuse rows[0] as the single row buffer.
                for y in y_start..y_end {
                    compute_ypbpr_row(xtrans, green_dir, colors, green_base, y, &mut rows[0]);

                    let drv_off = drv_base + y * width;
                    for x in 0..width {
                        let (yc, pbc, prc) = rows[0][x];

                        let (yf, pbf, prf) = if x + 1 < width {
                            rows[0][x + 1]
                        } else {
                            (yc, pbc, prc)
                        };

                        let (yb, pbb, prb) = if x > 0 {
                            rows[0][x - 1]
                        } else {
                            (yc, pbc, prc)
                        };

                        let dy = 2.0 * yc - yf - yb;
                        let dpb = 2.0 * pbc - pbf - pbb;
                        let dpr = 2.0 * prc - prf - prb;

                        // SAFETY: Each (d, chunk) pair writes to unique rows in drv.
                        unsafe {
                            *drv_ptr.get().add(drv_off + x) = dy * dy + dpb * dpb + dpr * dpr;
                        }
                    }
                }
            } else {
                // Directions 1,2,3 (dir_dy=1): sliding 3-row cache.
                // rows[0] = prev (y-1), rows[1] = center (y), rows[2] = next (y+1)

                // Seed the sliding window for this chunk
                if y_start > 0 {
                    compute_ypbpr_row(
                        xtrans,
                        green_dir,
                        colors,
                        green_base,
                        y_start - 1,
                        &mut rows[0],
                    );
                }
                compute_ypbpr_row(xtrans, green_dir, colors, green_base, y_start, &mut rows[1]);
                if y_start + 1 < height {
                    compute_ypbpr_row(
                        xtrans,
                        green_dir,
                        colors,
                        green_base,
                        y_start + 1,
                        &mut rows[2],
                    );
                }

                for y in y_start..y_end {
                    let has_prev = y > 0;
                    let has_next = y + 1 < height;

                    let drv_off = drv_base + y * width;
                    for x in 0..width {
                        let (yc, pbc, prc) = rows[1][x];

                        let fx = x as i32 + dir_dx;
                        let (yf, pbf, prf) = if has_next && fx >= 0 && (fx as usize) < width {
                            rows[2][fx as usize]
                        } else {
                            (yc, pbc, prc)
                        };

                        let bx = x as i32 - dir_dx;
                        let (yb, pbb, prb) = if has_prev && bx >= 0 && (bx as usize) < width {
                            rows[0][bx as usize]
                        } else {
                            (yc, pbc, prc)
                        };

                        let dy = 2.0 * yc - yf - yb;
                        let dpb = 2.0 * pbc - pbf - pbb;
                        let dpr = 2.0 * prc - prf - prb;

                        unsafe {
                            *drv_ptr.get().add(drv_off + x) = dy * dy + dpb * dpb + dpr * dpr;
                        }
                    }

                    // Slide window: prev <- center <- next, compute new next row
                    let [r0, r1, r2] = rows;
                    std::mem::swap(r0, r1);
                    std::mem::swap(r1, r2);
                    if y + 2 < height {
                        compute_ypbpr_row(xtrans, green_dir, colors, green_base, y + 2, r2);
                    }
                }
            }
        },
    );
}

/// Pre-compute YPbPr values for an entire row, storing results in `out`.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn compute_ypbpr_row(
    xtrans: &XTransImage,
    green_dir: &[f32],
    colors: &[[f32; 2]],
    green_base: usize,
    y: usize,
    out: &mut [(f32, f32, f32)],
) {
    for (x, val) in out.iter_mut().enumerate() {
        let index = green_base + y * xtrans.width + x;
        let [r, b] = colors[index];
        let g = green_dir[index];
        *val = rgb_to_ypbpr(r, g, b);
    }
}

/// Convert RGB to YPbPr.
#[inline(always)]
fn rgb_to_ypbpr(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let luma = 0.2627 * r + 0.6780 * g + 0.0593 * b;
    let pb = (b - luma) * 0.56433;
    let pr = (r - luma) * 0.67815;
    (luma, pb, pr)
}

/// Build homogeneity maps from per-direction derivatives.
///
/// Two sub-passes:
/// 1. Find minimum derivative across all 4 directions at each pixel → threshold = 8 × min
/// 2. In a 3×3 window, count how many pixels have drv ≤ threshold
pub(crate) fn compute_homogeneity(
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
                let center_threshold = threshold[y * width + x];
                for vy in y - 1..=y + 1 {
                    for vx in x - 1..=x + 1 {
                        let nidx = vy * width + vx;
                        if drv[d * pixels + nidx] <= center_threshold {
                            count += 1;
                        }
                    }
                }
                *homo_val = count;
            }
        });
}

/// Build an inclusive summed-area table in exactly one `width × height` plane.
fn build_summed_area_table(data: &[u8], width: usize, height: usize, sat: &mut [u32]) {
    debug_assert_eq!(data.len(), width * height);
    debug_assert_eq!(sat.len(), width * height);
    for y in 0..height {
        let mut row_sum = 0u32;
        for x in 0..width {
            row_sum += data[y * width + x] as u32;
            let above = if y == 0 { 0 } else { sat[(y - 1) * width + x] };
            sat[y * width + x] = row_sum + above;
        }
    }
}

/// Query a rectangular sum from a summed area table.
/// Computes sum of data[y0..=y1][x0..=x1] in O(1).
#[inline(always)]
fn sat_query(sat: &[u32], width: usize, y0: usize, x0: usize, y1: usize, x1: usize) -> u32 {
    let bottom_right = sat[y1 * width + x1];
    let above = if y0 == 0 {
        0
    } else {
        sat[(y0 - 1) * width + x1]
    };
    let left = if x0 == 0 { 0 } else { sat[y1 * width + x0 - 1] };
    let above_left = if y0 == 0 || x0 == 0 {
        0
    } else {
        sat[(y0 - 1) * width + x0 - 1]
    };
    bottom_right + above_left - above - left
}

fn score_homogeneity(
    homo: &[u8],
    width: usize,
    height: usize,
    scores: &mut [[u32; NDIR]],
    sat: &mut [u32],
) {
    let pixels = width * height;
    debug_assert_eq!(homo.len(), NDIR * pixels);
    debug_assert_eq!(scores.len(), pixels);
    debug_assert_eq!(sat.len(), pixels);

    for d in 0..NDIR {
        build_summed_area_table(&homo[d * pixels..(d + 1) * pixels], width, height, sat);

        for y in 0..height {
            let y0 = y.saturating_sub(2);
            let y1 = (y + 2).min(height - 1);
            for x in 0..width {
                let x0 = x.saturating_sub(2);
                let x1 = (x + 2).min(width - 1);
                scores[y * width + x][d] = sat_query(sat, width, y0, x0, y1, x1);
            }
        }
    }
}

/// Final blending: sum homogeneity in 5×5 window, select best directions,
/// and average the materialized RGB candidates.
///
/// Uses a summed-area table for O(1) per-pixel window queries instead of O(25).
///
/// `out_r`/`out_g`/`out_b` are preallocated planar channels (each length `pixels`) that the
/// final RGB is written into.
#[allow(clippy::too_many_arguments)]
pub(crate) fn blend_final(
    xtrans: &XTransImage,
    green_dir: &[f32],
    colors: &[[f32; 2]],
    homo: &[u8],
    scores: &mut [[u32; NDIR]],
    sat: &mut [u32],
    out_r: &mut [f32],
    out_g: &mut [f32],
    out_b: &mut [f32],
) {
    let width = xtrans.width;
    let height = xtrans.height;
    let pixels = width * height;
    assert_eq!(out_r.len(), pixels);
    assert_eq!(out_g.len(), pixels);
    assert_eq!(out_b.len(), pixels);

    score_homogeneity(homo, width, height, scores, sat);

    out_r
        .par_chunks_mut(width)
        .zip(out_g.par_chunks_mut(width))
        .zip(out_b.par_chunks_mut(width))
        .enumerate()
        .for_each(|(y, ((r_row, g_row), b_row))| {
            for x in 0..width {
                let hm = &scores[y * width + x];

                // Find max homogeneity score
                let max_hm = *hm.iter().max().unwrap();
                let threshold = max_hm - (max_hm >> 3); // 7/8 of max

                // Average RGB from qualifying directions.
                let mut avg_r = 0.0f32;
                let mut avg_g = 0.0f32;
                let mut avg_b = 0.0f32;
                let mut count = 0u32;
                for (d, &h) in hm.iter().enumerate() {
                    if h >= threshold {
                        let green_base = d * pixels;
                        let index = green_base + y * width + x;
                        let [r, b] = colors[index];
                        avg_r += r;
                        avg_g += green_dir[index];
                        avg_b += b;
                        count += 1;
                    }
                }

                debug_assert!(count > 0);
                let inv = 1.0 / count as f32;
                r_row[x] = avg_r * inv;
                g_row[x] = avg_g * inv;
                b_row[x] = avg_b * inv;
            }
        });

    demosaic_border(xtrans, out_r, out_g, out_b, MARK_INFO_BORDER);
}

fn demosaic_border(
    xtrans: &XTransImage,
    out_r: &mut [f32],
    out_g: &mut [f32],
    out_b: &mut [f32],
    border: usize,
) {
    let width = xtrans.width;
    let height = xtrans.height;

    for y in 0..height {
        for x in 0..width {
            if y >= border && y + border < height && x >= border && x + border < width {
                continue;
            }

            let mut sums = [0.0f32; 3];
            let mut weights = [0.0f32; 3];
            for neighbor_y in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                for neighbor_x in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                    let dy = neighbor_y.abs_diff(y);
                    let dx = neighbor_x.abs_diff(x);
                    let weight = match (dy, dx) {
                        (0, 0) => 0.0,
                        (0, 1) | (1, 0) => 0.5,
                        (1, 1) => 0.25,
                        _ => unreachable!(),
                    };
                    let color = xtrans.raw_pattern.color_at(
                        neighbor_y + xtrans.top_margin,
                        neighbor_x + xtrans.left_margin,
                    ) as usize;
                    sums[color] += active_raw(xtrans, neighbor_y, neighbor_x) * weight;
                    weights[color] += weight;
                }
            }

            let index = y * width + x;
            let native = xtrans
                .raw_pattern
                .color_at(y + xtrans.top_margin, x + xtrans.left_margin);
            let raw = active_raw(xtrans, y, x);
            if native == 1 && weights[0] == 0.0 {
                out_r[index] = raw;
                out_g[index] = raw;
                out_b[index] = raw;
                continue;
            }
            out_r[index] = if native == 0 || weights[0] == 0.0 {
                raw
            } else {
                sums[0] / weights[0]
            };
            out_g[index] = if native == 1 || weights[1] == 0.0 {
                raw
            } else {
                sums[1] / weights[1]
            };
            out_b[index] = if native == 2 || weights[2] == 0.0 {
                raw
            } else {
                sums[2] / weights[2]
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::io::raw::demosaic::interleave_planes;
    use crate::io::raw::demosaic::xtrans::hex_lookup::HexLookup;
    use crate::io::raw::demosaic::xtrans::markesteijn_steps::*;
    use crate::io::raw::demosaic::xtrans::test_support::{make_xtrans, test_pattern, to_u16};

    #[test]
    fn test_green_minmax_uniform() {
        let raw_w = 24;
        let raw_h = 24;
        let w = 12;
        let h = 12;
        let data = vec![to_u16(0.5); raw_w * raw_h];
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);
        let hex = HexLookup::new(&xtrans.raw_pattern);

        let mut gmin = vec![0.0f32; w * h];
        let mut gmax = vec![0.0f32; w * h];
        compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);

        // Uniform 0.5 input → gmin=gmax≈0.5 everywhere (u16 quantization: ±1e-5)
        for i in 0..w * h {
            assert!((gmin[i] - 0.5).abs() < 1e-4, "gmin[{}] = {}", i, gmin[i]);
            assert!((gmax[i] - 0.5).abs() < 1e-4, "gmax[{}] = {}", i, gmax[i]);
        }
    }

    #[test]
    fn test_green_minmax_bounds() {
        let raw_w = 24;
        let raw_h = 24;
        let w = 12;
        let h = 12;
        // Create gradient data
        let data: Vec<u16> = (0..raw_w * raw_h)
            .map(|i| to_u16((i as f32) / (raw_w * raw_h) as f32))
            .collect();
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);
        let hex = HexLookup::new(&xtrans.raw_pattern);

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
        let data = vec![to_u16(0.5); raw_w * raw_h];
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);
        let hex = HexLookup::new(&xtrans.raw_pattern);

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
    fn homogeneity_uses_the_center_threshold_for_the_entire_window() {
        let width = 3;
        let height = 3;
        let pixels = width * height;
        let center = 4;
        let mut drv = vec![100.0f32; NDIR * pixels];
        drv[..pixels].copy_from_slice(&[1.0, 2.0, 3.0, 4.0, 1.0, 5.0, 6.0, 7.0, 8.0]);
        drv[pixels..2 * pixels].fill(0.1);
        for direction in 1..NDIR {
            drv[direction * pixels + center] = 2.0;
        }
        let mut homo = vec![0u8; NDIR * pixels];
        let mut threshold = vec![0.0f32; pixels];

        compute_homogeneity(&drv, width, height, &mut homo, &mut threshold);

        assert_eq!(threshold[center], 8.0);
        assert_eq!(homo[center], 9);
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
    fn test_ypbpr_conversion_primary_colors() {
        // Pure red (1,0,0): Y=0.2627, Pb=-0.2627*0.56433, Pr=0.7373*0.67815
        let (y, pb, pr) = rgb_to_ypbpr(1.0, 0.0, 0.0);
        assert!((y - 0.2627).abs() < 1e-6, "Red Y={y}");
        assert!((pb - (-0.2627 * 0.56433)).abs() < 1e-6, "Red Pb={pb}");
        assert!((pr - (0.7373 * 0.67815)).abs() < 1e-4, "Red Pr={pr}");

        // Pure green (0,1,0): Y=0.6780, Pb=-0.6780*0.56433, Pr=-0.6780*0.67815
        let (y, pb, pr) = rgb_to_ypbpr(0.0, 1.0, 0.0);
        assert!((y - 0.6780).abs() < 1e-6, "Green Y={y}");
        assert!((pb - (-0.6780 * 0.56433)).abs() < 1e-6, "Green Pb={pb}");
        assert!((pr - (-0.6780 * 0.67815)).abs() < 1e-4, "Green Pr={pr}");

        // Pure blue (0,0,1): Y=0.0593, Pb=0.9407*0.56433, Pr=-0.0593*0.67815
        let (y, pb, pr) = rgb_to_ypbpr(0.0, 0.0, 1.0);
        assert!((y - 0.0593).abs() < 1e-6, "Blue Y={y}");
        assert!((pb - (0.9407 * 0.56433)).abs() < 1e-4, "Blue Pb={pb}");
        assert!((pr - (-0.0593 * 0.67815)).abs() < 1e-4, "Blue Pr={pr}");

        // Mid-gray (0.5, 0.5, 0.5): Y=0.5, Pb=0, Pr=0
        let (y, pb, pr) = rgb_to_ypbpr(0.5, 0.5, 0.5);
        assert!((y - 0.5).abs() < 1e-6, "Gray Y={y}");
        assert!(pb.abs() < 1e-6, "Gray Pb={pb}");
        assert!(pr.abs() < 1e-6, "Gray Pr={pr}");
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

    #[test]
    fn test_derivatives_of_uniform_input_are_finite_and_expose_directional_candidates() {
        let raw_w = 24;
        let raw_h = 24;
        let w = 12;
        let h = 12;
        let pixels = w * h;
        let data = vec![to_u16(0.5); raw_w * raw_h];
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);
        let hex = HexLookup::new(&xtrans.raw_pattern);

        let mut gmin = vec![0.0f32; pixels];
        let mut gmax = vec![1.0f32; pixels];
        compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);

        let mut green_dir = vec![0.0f32; NDIR * pixels];
        interpolate_green(&xtrans, &hex, &gmin, &gmax, &mut green_dir);

        let mut colors = vec![[0.0; 2]; NDIR * pixels];
        reconstruct_colors(&xtrans, &hex, &green_dir, &mut colors);
        let mut drv = vec![f32::NAN; NDIR * pixels];
        compute_derivatives(&xtrans, &green_dir, &colors, &mut drv);

        let mut nonzero = [0usize; NDIR];
        for d in 0..NDIR {
            for y in 2..h - 2 {
                for x in 2..w - 2 {
                    let val = drv[d * pixels + y * w + x];
                    assert!(val.is_finite(), "NaN derivative at d={d} y={y} x={x}");
                    assert!(val >= 0.0, "Negative derivative at d={d} y={y} x={x}");
                    nonzero[d] += usize::from(val > 1e-6);
                }
            }
        }
        assert!(nonzero[0] > 0);
        assert_ne!(nonzero[0], nonzero[2]);
    }

    #[test]
    fn test_derivatives_checkerboard_nonzero() {
        // Checkerboard input has sharp edges → non-zero Laplacian (derivatives).
        let raw_w = 24;
        let raw_h = 24;
        let w = 12;
        let h = 12;
        let pixels = w * h;
        let data: Vec<u16> = (0..raw_w * raw_h)
            .map(|i| {
                let y = i / raw_w;
                let x = i % raw_w;
                if (x + y) % 2 == 0 {
                    to_u16(0.8)
                } else {
                    to_u16(0.2)
                }
            })
            .collect();
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);
        let hex = HexLookup::new(&xtrans.raw_pattern);

        let mut gmin = vec![0.0f32; pixels];
        let mut gmax = vec![1.0f32; pixels];
        compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);

        let mut green_dir = vec![0.0f32; NDIR * pixels];
        interpolate_green(&xtrans, &hex, &gmin, &gmax, &mut green_dir);

        let mut colors = vec![[0.0; 2]; NDIR * pixels];
        reconstruct_colors(&xtrans, &hex, &green_dir, &mut colors);
        let mut drv = vec![0.0f32; NDIR * pixels];
        compute_derivatives(&xtrans, &green_dir, &colors, &mut drv);

        // All derivatives should be finite and non-negative (squared values)
        for (i, &val) in drv.iter().enumerate() {
            assert!(val.is_finite(), "NaN derivative at index {i}");
            assert!(val >= 0.0, "Negative derivative at index {i}: {val}");
        }

        // Checkerboard has high-frequency content → some derivatives must be non-zero
        let mut nonzero_count = 0;
        for d in 0..NDIR {
            for y in 2..h - 2 {
                for x in 2..w - 2 {
                    if drv[d * pixels + y * w + x] > 1e-6 {
                        nonzero_count += 1;
                    }
                }
            }
        }
        assert!(
            nonzero_count > 0,
            "Expected some non-zero derivatives for checkerboard input"
        );
    }

    #[test]
    fn test_sat_uniform_ones() {
        // 4×3 grid of all 1s
        let data = vec![1u8; 4 * 3];
        let mut sat = vec![u32::MAX; data.len()];
        build_summed_area_table(&data, 4, 3, &mut sat);
        assert_eq!(sat, [1, 2, 3, 4, 2, 4, 6, 8, 3, 6, 9, 12]);

        // Full image sum = 12
        assert_eq!(sat_query(&sat, 4, 0, 0, 2, 3), 12);
        // Single pixel (0,0) = 1
        assert_eq!(sat_query(&sat, 4, 0, 0, 0, 0), 1);
        // First row sum = 4
        assert_eq!(sat_query(&sat, 4, 0, 0, 0, 3), 4);
        // First column sum = 3
        assert_eq!(sat_query(&sat, 4, 0, 0, 2, 0), 3);
        // 2×2 top-left corner = 4
        assert_eq!(sat_query(&sat, 4, 0, 0, 1, 1), 4);
        // 2×2 bottom-right corner = 4
        assert_eq!(sat_query(&sat, 4, 1, 2, 2, 3), 4);
    }

    #[test]
    fn test_sat_sequential_values() {
        // 3×3 grid: [1,2,3; 4,5,6; 7,8,9]
        let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9];
        let mut sat = vec![u32::MAX; data.len()];
        build_summed_area_table(&data, 3, 3, &mut sat);

        // Full sum = 45
        assert_eq!(sat_query(&sat, 3, 0, 0, 2, 2), 45);
        // Center pixel only = 5
        assert_eq!(sat_query(&sat, 3, 1, 1, 1, 1), 5);
        // Middle row = 4+5+6 = 15
        assert_eq!(sat_query(&sat, 3, 1, 0, 1, 2), 15);
        // Bottom-right 2×2 = 5+6+8+9 = 28
        assert_eq!(sat_query(&sat, 3, 1, 1, 2, 2), 28);
    }

    #[test]
    fn test_sat_single_pixel() {
        let data = vec![42u8];
        let mut sat = vec![u32::MAX; data.len()];
        build_summed_area_table(&data, 1, 1, &mut sat);
        assert_eq!(sat_query(&sat, 1, 0, 0, 0, 0), 42);
    }

    #[test]
    fn test_sat_single_row() {
        let data = vec![1, 2, 3, 4, 5];
        let mut sat = vec![u32::MAX; data.len()];
        build_summed_area_table(&data, 5, 1, &mut sat);
        // Full row = 15
        assert_eq!(sat_query(&sat, 5, 0, 0, 0, 4), 15);
        // Middle 3 elements = 2+3+4 = 9
        assert_eq!(sat_query(&sat, 5, 0, 1, 0, 3), 9);
    }

    #[test]
    fn test_sat_single_column() {
        let data = vec![1, 2, 3, 4, 5];
        let mut sat = vec![u32::MAX; data.len()];
        build_summed_area_table(&data, 1, 5, &mut sat);
        // Full column = 15
        assert_eq!(sat_query(&sat, 1, 0, 0, 4, 0), 15);
        // Middle 3 = 2+3+4 = 9
        assert_eq!(sat_query(&sat, 1, 1, 0, 3, 0), 9);
    }

    #[test]
    fn test_sat_zeros() {
        let data = vec![0u8; 4 * 4];
        let mut sat = vec![u32::MAX; data.len()];
        build_summed_area_table(&data, 4, 4, &mut sat);
        assert_eq!(sat_query(&sat, 4, 0, 0, 3, 3), 0);
    }

    #[test]
    fn homogeneity_scores_match_direct_five_by_five_windows() {
        let width = 6;
        let height = 5;
        let pixels = width * height;
        let mut homo = vec![0u8; NDIR * pixels];
        for direction in 0..NDIR {
            for y in 0..height {
                for x in 0..width {
                    homo[direction * pixels + y * width + x] =
                        ((3 * direction + 2 * y + x) % 10) as u8;
                }
            }
        }
        let mut scores = vec![[u32::MAX; NDIR]; pixels];
        let mut sat = vec![u32::MAX; pixels];

        score_homogeneity(&homo, width, height, &mut scores, &mut sat);

        for direction in 0..NDIR {
            for y in 0..height {
                for x in 0..width {
                    let mut expected = 0u32;
                    for sample_y in y.saturating_sub(2)..=(y + 2).min(height - 1) {
                        for sample_x in x.saturating_sub(2)..=(x + 2).min(width - 1) {
                            expected +=
                                homo[direction * pixels + sample_y * width + sample_x] as u32;
                        }
                    }
                    assert_eq!(scores[y * width + x][direction], expected);
                }
            }
        }
    }

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

    #[test]
    fn geometry_stages_cover_each_xtrans_site() {
        let pattern = test_pattern();
        let hex = HexLookup::new(&pattern);
        let mut solitary = 0;
        let mut colored = 0;
        let mut green_block = 0;

        for y in 0..6 {
            for x in 0..6 {
                match pattern.color_at(y, x) {
                    0 | 2 => colored += 1,
                    1 if is_solitary_green(&hex, y, x) => solitary += 1,
                    1 => green_block += 1,
                    _ => unreachable!(),
                }
            }
        }

        assert_eq!(solitary, 4);
        assert_eq!(colored, 16);
        assert_eq!(green_block, 16);
    }

    #[test]
    fn reconstruction_preserves_native_samples_and_canonical_empty_directions() {
        let raw_w = 30;
        let raw_h = 30;
        let w = 18;
        let h = 18;
        let pixels = w * h;
        let data = vec![to_u16(0.5); raw_w * raw_h];
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);
        let hex = HexLookup::new(&xtrans.raw_pattern);
        let mut gmin = vec![0.0; pixels];
        let mut gmax = vec![0.0; pixels];
        let mut green_dir = vec![0.0; NDIR * pixels];
        let mut colors = vec![[f32::NAN; 2]; NDIR * pixels];
        compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);
        interpolate_green(&xtrans, &hex, &gmin, &gmax, &mut green_dir);
        reconstruct_colors(&xtrans, &hex, &green_dir, &mut colors);

        for direction in 0..NDIR {
            for y in 3..h - 3 {
                for x in 3..w - 3 {
                    let raw_y = y + xtrans.top_margin;
                    let raw_x = x + xtrans.left_margin;
                    let native = xtrans.raw_pattern.color_at(raw_y, raw_x);
                    let [red, blue] = colors[direction * pixels + y * w + x];
                    match native {
                        0 => assert_eq!(red, active_raw(&xtrans, y, x)),
                        2 => assert_eq!(blue, active_raw(&xtrans, y, x)),
                        1 if !is_solitary_green(&hex, raw_y, raw_x) && direction >= 2 => {
                            assert_eq!([red, blue], [0.0, 0.0]);
                            continue;
                        }
                        1 => {}
                        _ => unreachable!(),
                    }
                    assert!(red.is_finite(), "d={direction} ({y},{x}) R={red}");
                    assert!(blue.is_finite(), "d={direction} ({y},{x}) B={blue}");
                }
            }
        }
    }

    #[test]
    fn reconstruction_geometry_dependencies_are_completed_by_earlier_stages() {
        let pattern = test_pattern();
        let hex = HexLookup::new(&pattern);

        for y in 3..9 {
            for x in 3..9 {
                let native = pattern.color_at(y, x);
                if native == 1 && !is_solitary_green(&hex, y, x) {
                    let offsets = hex.get(y, x);
                    for offset in &offsets[..4] {
                        let neighbor_y = y.wrapping_add_signed(offset.dy as isize);
                        let neighbor_x = x.wrapping_add_signed(offset.dx as isize);
                        let neighbor = pattern.color_at(neighbor_y, neighbor_x);
                        assert!(
                            neighbor != 1 || is_solitary_green(&hex, neighbor_y, neighbor_x),
                            "2x2-green dependency at ({y},{x}) reaches another block at \
                             ({neighbor_y},{neighbor_x})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_blend_uniform_homo_produces_uniform_output() {
        // With uniform input and uniform homogeneity, output should be uniform
        let raw_w = 24;
        let raw_h = 24;
        let w = 12;
        let h = 12;
        let pixels = w * h;
        let data = vec![to_u16(0.5); raw_w * raw_h];
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);
        let hex = HexLookup::new(&xtrans.raw_pattern);

        let mut gmin = vec![0.0f32; pixels];
        let mut gmax = vec![1.0f32; pixels];
        compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);

        let mut green_dir = vec![0.0f32; NDIR * pixels];
        interpolate_green(&xtrans, &hex, &gmin, &gmax, &mut green_dir);
        let mut colors = vec![[0.0; 2]; NDIR * pixels];
        reconstruct_colors(&xtrans, &hex, &green_dir, &mut colors);

        // Uniform homogeneity → all directions qualify
        let homo = vec![9u8; NDIR * pixels];

        let mut r = vec![0.0f32; pixels];
        let mut g = vec![0.0f32; pixels];
        let mut b = vec![0.0f32; pixels];
        let mut scores = vec![[0; NDIR]; pixels];
        let mut sat = vec![0; pixels];
        blend_final(
            &xtrans,
            &green_dir,
            &colors,
            &homo,
            &mut scores,
            &mut sat,
            &mut r,
            &mut g,
            &mut b,
        );

        // Uniform 0.5 input → output should be approximately 0.5 for all channels
        let output = interleave_planes([r, g, b]);
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
        let data = vec![to_u16(0.5); raw_w * raw_h];
        let xtrans = make_xtrans(&data, raw_w, raw_h, w, h, 6, 6);
        let hex = HexLookup::new(&xtrans.raw_pattern);

        let mut gmin = vec![0.0f32; pixels];
        let mut gmax = vec![1.0f32; pixels];
        compute_green_minmax(&xtrans, &hex, &mut gmin, &mut gmax);

        let mut green_dir = vec![0.0f32; NDIR * pixels];
        interpolate_green(&xtrans, &hex, &gmin, &gmax, &mut green_dir);
        let mut colors = vec![[0.0; 2]; NDIR * pixels];
        reconstruct_colors(&xtrans, &hex, &green_dir, &mut colors);

        // Dir 0 has highest homogeneity (9), others have 0
        let mut homo = vec![0u8; NDIR * pixels];
        homo[..pixels].fill(9);

        let mut r_one = vec![0.0f32; pixels];
        let mut g_one = vec![0.0f32; pixels];
        let mut b_one = vec![0.0f32; pixels];
        let mut scores = vec![[0; NDIR]; pixels];
        let mut sat = vec![0; pixels];
        blend_final(
            &xtrans,
            &green_dir,
            &colors,
            &homo,
            &mut scores,
            &mut sat,
            &mut r_one,
            &mut g_one,
            &mut b_one,
        );
        let output_one = interleave_planes([r_one, g_one, b_one]);

        // All directions equally good
        let homo_all = vec![9u8; NDIR * pixels];
        let mut r_all = vec![0.0f32; pixels];
        let mut g_all = vec![0.0f32; pixels];
        let mut b_all = vec![0.0f32; pixels];
        blend_final(
            &xtrans,
            &green_dir,
            &colors,
            &homo_all,
            &mut scores,
            &mut sat,
            &mut r_all,
            &mut g_all,
            &mut b_all,
        );
        let output_all = interleave_planes([r_all, g_all, b_all]);

        let mut changed = false;
        for y in MARK_INFO_BORDER..h - MARK_INFO_BORDER {
            for x in MARK_INFO_BORDER..w - MARK_INFO_BORDER {
                let pixel = y * w + x;
                let [expected_r, expected_b] = colors[pixel];
                let expected_g = green_dir[pixel];
                assert_eq!(
                    &output_one[pixel * 3..pixel * 3 + 3],
                    &[expected_r, expected_g, expected_b]
                );
                changed |=
                    output_one[pixel * 3..pixel * 3 + 3] != output_all[pixel * 3..pixel * 3 + 3];
            }
        }
        assert!(changed);

        // Output should have no NaN or negative values
        for (i, &v) in output_one.iter().enumerate() {
            assert!(v.is_finite(), "NaN at {i}");
            assert!(v >= 0.0, "Negative at {i}");
        }
    }
}
