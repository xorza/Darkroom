//! The frame's border, which no tile reaches: each pixel's missing colours from its 3×3
//! neighbours, weighted by distance.

use crate::io::raw::demosaic::xtrans::XTransImage;
use crate::math::vec2us::Vec2us;

/// Fill the pixels within `border` of the frame's edge of `out`.
pub(super) fn fill(
    xtrans: &XTransImage<'_>,
    [out_r, out_g, out_b]: [&mut [f32]; 3],
    border: usize,
) {
    let width = xtrans.size.width;
    let height = xtrans.size.height;

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
                    let color =
                        xtrans.pattern.color_at(Vec2us::new(neighbor_x, neighbor_y)) as usize;
                    sums[color] += xtrans.read(neighbor_y, neighbor_x) * weight;
                    weights[color] += weight;
                }
            }

            let index = y * width + x;
            let native = xtrans.pattern.color_at(Vec2us::new(x, y)) as usize;
            let raw = xtrans.read(y, x);
            let channel = |color: usize| {
                if color == native {
                    raw
                } else if weights[color] > 0.0 {
                    sums[color] / weights[color]
                } else {
                    nearest_same_color_mean(xtrans, y, x, color).unwrap_or(raw)
                }
            };
            out_r[index] = channel(0);
            out_g[index] = channel(1);
            out_b[index] = channel(2);
        }
    }
}

/// The mean of `color`'s samples in the smallest square window around `(x, y)` that holds any,
/// for a border pixel whose 3×3 neighbourhood has none. `None` only when the whole frame has no
/// sample of `color`, where there is nothing to interpolate from and the caller keeps the pixel's
/// own sample.
fn nearest_same_color_mean(
    xtrans: &XTransImage<'_>,
    y: usize,
    x: usize,
    color: usize,
) -> Option<f32> {
    let width = xtrans.size.width;
    let height = xtrans.size.height;
    (2..width.max(height)).find_map(|radius| {
        let mut sum = 0.0f32;
        let mut count = 0usize;
        for neighbor_y in y.saturating_sub(radius)..=(y + radius).min(height - 1) {
            for neighbor_x in x.saturating_sub(radius)..=(x + radius).min(width - 1) {
                let neighbor_color =
                    xtrans.pattern.color_at(Vec2us::new(neighbor_x, neighbor_y)) as usize;
                if neighbor_color == color {
                    sum += xtrans.read(neighbor_y, neighbor_x);
                    count += 1;
                }
            }
        }
        (count > 0).then(|| sum / count as f32)
    })
}
