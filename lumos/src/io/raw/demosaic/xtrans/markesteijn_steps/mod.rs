//! Algorithm step implementations for Markesteijn 1-pass demosaicing.
//!
//! Each step is a separate function that operates on the shared working buffers.
//! Steps are called sequentially by the orchestrator in `markesteijn.rs`.

use rayon::prelude::*;

use crate::io::raw::demosaic::xtrans::XTransImage;
use crate::io::raw::demosaic::xtrans::hex_lookup::HexLookup;
use crate::io::raw::demosaic::xtrans::markesteijn::NDIR;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

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
    let width = xtrans.active.width;
    let height = xtrans.active.height;
    assert_eq!(gmin.len(), width * height);
    assert_eq!(gmax.len(), width * height);

    gmin.par_chunks_mut(width)
        .zip(gmax.par_chunks_mut(width))
        .enumerate()
        .for_each(|(y, (gmin_row, gmax_row))| {
            let raw_y = y + xtrans.margin.y;
            for x in 0..width {
                let raw_x = x + xtrans.margin.x;
                let color = xtrans.raw_pattern.color_at(Vec2us::new(raw_x, raw_y));

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
                            && (ny as usize) < xtrans.raw.height
                            && (nx as usize) < xtrans.raw.width
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
    let width = xtrans.active.width;
    let height = xtrans.active.height;
    let pixels = width * height;
    let raw_width = xtrans.raw.width;

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
        let raw_y = y + xtrans.margin.y;
        let row_off = y * width;
        // librtprocess stores the alternating-row candidates in the opposite direction slots.
        let flip = ((raw_y as i64 - hex.sgrow as i64).rem_euclid(3) == 0) as usize;

        for x in 0..width {
            let raw_x = x + xtrans.margin.x;
            let color = xtrans.raw_pattern.color_at(Vec2us::new(raw_x, raw_y));

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
                        && (ny as usize) < xtrans.raw.height
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
    xtrans.read_normalized(y + xtrans.margin.y, x + xtrans.margin.x)
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

#[inline(always)]
fn solitary_green_candidate(
    xtrans: &XTransImage,
    green_dir: &[f32],
    green_base: usize,
    y: usize,
    x: usize,
    candidate: usize,
) -> SolitaryGreenCandidate {
    let width = xtrans.active.width;
    let (dy, dx) = if candidate & 1 == 0 {
        (0isize, 1isize)
    } else {
        (1, 0)
    };
    let center_green = green_at(green_dir, green_base, width, y, x);
    let raw_y = y + xtrans.margin.y;
    let raw_x = x + xtrans.margin.x;
    let mut colors = [0.0; 2];
    let mut difference = 0.0;
    let mut target = xtrans.raw_pattern.color_at(Vec2us::new(raw_x + 1, raw_y));
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
        let plus_native = xtrans.raw_pattern.color_at(Vec2us::new(
            raw_x.wrapping_add_signed(ox),
            raw_y.wrapping_add_signed(oy),
        ));
        let minus_native = xtrans.raw_pattern.color_at(Vec2us::new(
            raw_x.wrapping_add_signed(-ox),
            raw_y.wrapping_add_signed(-oy),
        ));
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
    pos: Vec2us,
    target: u8,
) -> f32 {
    // SAFETY: Initialization and the solitary-green stage are complete before this stage.
    unsafe { (*colors.add(direction * pixels + pos.y * width + pos.x))[rb_index(target)] }
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
    let width = xtrans.active.width;
    let raw_y = y + xtrans.margin.y;
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
    let plus = Vec2us::new(plus_x, plus_y);
    let minus = Vec2us::new(minus_x, minus_y);
    let color_plus = color_before_opposite(colors, pixels, direction, width, plus, target);
    let color_minus = color_before_opposite(colors, pixels, direction, width, minus, target);
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
        .color_at(Vec2us::new(x + xtrans.margin.x, y + xtrans.margin.y));
    debug_assert!(native != 1 || is_solitary_green(hex, y + xtrans.margin.y, x + xtrans.margin.x));
    if native == target {
        active_raw(xtrans, y, x)
    } else {
        let pixels = xtrans.active.pixel_count();
        // SAFETY: The solitary-green and opposite-color stages are complete before this stage.
        unsafe { (*colors.add(direction * pixels + y * xtrans.active.width + x))[rb_index(target)] }
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

    let width = xtrans.active.width;
    let offsets = hex.get(y + xtrans.margin.y, x + xtrans.margin.x);
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
    let width = xtrans.active.width;
    let height = xtrans.active.height;
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
            let raw_y = y + xtrans.margin.y;
            let raw_x = x + xtrans.margin.x;
            let native = xtrans.raw_pattern.color_at(Vec2us::new(raw_x, raw_y));
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
        let raw_y = y + xtrans.margin.y;
        let raw_x = x + xtrans.margin.x;
        let native = xtrans.raw_pattern.color_at(Vec2us::new(raw_x, raw_y));
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
            let raw_y = y + xtrans.margin.y;
            let raw_x = x + xtrans.margin.x;
            if xtrans.raw_pattern.color_at(Vec2us::new(raw_x, raw_y)) != 1
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
    let width = xtrans.active.width;
    let height = xtrans.active.height;
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
            // Allocate 3 row buffers once per rayon thread, reused across chunks. Allocated in
            // the init rather than leased from a `JobScratchPool`: a demosaic pass runs this loop
            // once and nothing in the RAW decode path outlives a single frame, so there is
            // nowhere warm to hand them back to.
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
        let index = green_base + y * xtrans.active.width + x;
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
    size: Size2us,
    homo: &mut [u8],
    threshold: &mut [f32],
) {
    let pixels = size.pixel_count();

    // Sub-pass 1: compute min derivative across directions and threshold
    threshold
        .par_chunks_mut(size.width)
        .enumerate()
        .for_each(|(y, thr_row)| {
            for (x, thr) in thr_row.iter_mut().enumerate() {
                let idx = y * size.width + x;
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
    homo.par_chunks_mut(size.width)
        .enumerate()
        .for_each(|(flat_idx, homo_row)| {
            let d = flat_idx / size.height;
            let y = flat_idx % size.height;

            if y == 0 || y >= size.height - 1 {
                homo_row.fill(0);
                return;
            }

            // First and last columns are border pixels
            homo_row[0] = 0;
            if size.width > 1 {
                homo_row[size.width - 1] = 0;
            }

            for (x, homo_val) in homo_row
                .iter_mut()
                .enumerate()
                .skip(1)
                .take(size.width.saturating_sub(2))
            {
                let mut count = 0u8;
                let center_threshold = threshold[y * size.width + x];
                for vy in y - 1..=y + 1 {
                    for vx in x - 1..=x + 1 {
                        let nidx = vy * size.width + vx;
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
fn build_summed_area_table(data: &[u8], size: Size2us, sat: &mut [u32]) {
    debug_assert_eq!(data.len(), size.pixel_count());
    debug_assert_eq!(sat.len(), size.pixel_count());
    for y in 0..size.height {
        let mut row_sum = 0u32;
        for x in 0..size.width {
            row_sum += data[y * size.width + x] as u32;
            let above = if y == 0 {
                0
            } else {
                sat[(y - 1) * size.width + x]
            };
            sat[y * size.width + x] = row_sum + above;
        }
    }
}

/// Query a rectangular sum from a summed area table.
/// Computes sum of data[y0..=y1][x0..=x1] in O(1).
#[inline(always)]
/// `min` and `max` are both inclusive. Takes the corners loose rather than as a [`URect`] because
/// this runs once per direction per pixel and `URect::new` asserts its bounds in release.
fn sat_query(sat: &[u32], width: usize, min: Vec2us, max: Vec2us) -> u32 {
    let bottom_right = sat[max.y * width + max.x];
    let above = if min.y == 0 {
        0
    } else {
        sat[(min.y - 1) * width + max.x]
    };
    let left = if min.x == 0 {
        0
    } else {
        sat[max.y * width + min.x - 1]
    };
    let above_left = if min.y == 0 || min.x == 0 {
        0
    } else {
        sat[(min.y - 1) * width + min.x - 1]
    };
    bottom_right + above_left - above - left
}

fn score_homogeneity(homo: &[u8], size: Size2us, scores: &mut [[u32; NDIR]], sat: &mut [u32]) {
    let pixels = size.pixel_count();
    debug_assert_eq!(homo.len(), NDIR * pixels);
    debug_assert_eq!(scores.len(), pixels);
    debug_assert_eq!(sat.len(), pixels);

    for d in 0..NDIR {
        build_summed_area_table(&homo[d * pixels..(d + 1) * pixels], size, sat);

        for y in 0..size.height {
            let y0 = y.saturating_sub(2);
            let y1 = (y + 2).min(size.height - 1);
            for x in 0..size.width {
                let min = Vec2us::new(x.saturating_sub(2), y0);
                let max = Vec2us::new((x + 2).min(size.width - 1), y1);
                scores[size.index_of(Vec2us::new(x, y))][d] = sat_query(sat, size.width, min, max);
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
    let width = xtrans.active.width;
    let pixels = xtrans.active.pixel_count();
    assert_eq!(out_r.len(), pixels);
    assert_eq!(out_g.len(), pixels);
    assert_eq!(out_b.len(), pixels);

    score_homogeneity(homo, xtrans.active, scores, sat);

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
    let width = xtrans.active.width;
    let height = xtrans.active.height;

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
                    let color = xtrans.raw_pattern.color_at(Vec2us::new(
                        neighbor_x + xtrans.margin.x,
                        neighbor_y + xtrans.margin.y,
                    )) as usize;
                    sums[color] += active_raw(xtrans, neighbor_y, neighbor_x) * weight;
                    weights[color] += weight;
                }
            }

            let index = y * width + x;
            let native = xtrans
                .raw_pattern
                .color_at(Vec2us::new(x + xtrans.margin.x, y + xtrans.margin.y));
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
mod tests;
