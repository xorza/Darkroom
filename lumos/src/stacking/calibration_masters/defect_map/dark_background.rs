//! The smooth dark-current model hot-pixel detection measures against.
//!
//! A master dark is not flat: it carries gradients and amp glow that a global threshold would read
//! as thousands of point defects. Robust per-colour medians over a coarse tile grid, bilinearly
//! interpolated back to full resolution, describe that broad structure — and because the model
//! comes from tile medians rather than a pixel's own neighbours, a compact same-colour cluster of
//! genuinely hot pixels stays an outlier instead of becoming its own reference.

use common::CancelToken;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::io::image::cfa::CfaType;
use crate::math::statistics::median_mut;
use crate::math::vec2us::Vec2us;
use crate::stacking::calibration_masters::defect_map::DARK_BACKGROUND_TILE_SIZE;
use crate::stacking::calibration_masters::defect_map::sampling::collect_color_samples;
use crate::stacking::combine::error::Error;

#[derive(Debug, Clone, Copy)]
struct InterpolationSpan {
    lower: usize,
    upper: usize,
    fraction: f32,
}

#[derive(Debug, Clone, Copy)]
struct DarkTile {
    values: [f32; 3],
}

/// Smooth per-CFA-color dark-current model sampled from robust tile medians.
#[derive(Debug)]
pub(super) struct DarkBackground {
    tiles: Buffer2<DarkTile>,
    x_spans: Vec<InterpolationSpan>,
    y_spans: Vec<InterpolationSpan>,
}

impl DarkBackground {
    pub(super) fn fit(
        data: &Buffer2<f32>,
        cfa_type: Option<&CfaType>,
        cancel: &CancelToken,
    ) -> Result<Self, Error> {
        let width = data.width();
        let height = data.height();
        assert!(
            width > 0 && height > 0,
            "dark background needs non-zero dimensions"
        );
        let tiles_x = width.div_ceil(DARK_BACKGROUND_TILE_SIZE);
        let tiles_y = height.div_ceil(DARK_BACKGROUND_TILE_SIZE);
        let pattern = CfaType::or_mono(cfa_type);
        let num_colors = pattern.num_colors();

        let mut tiles: Vec<DarkTile> = (0..tiles_x * tiles_y)
            .into_par_iter()
            .map(|index| {
                if cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }

                let tx = index % tiles_x;
                let ty = index / tiles_x;
                let x_start = tx * width / tiles_x;
                let x_end = (tx + 1) * width / tiles_x;
                let y_start = ty * height / tiles_y;
                let y_end = (ty + 1) * height / tiles_y;
                let mut samples: [Vec<f32>; 3] = std::array::from_fn(|_| Vec::new());

                for y in y_start..y_end {
                    for x in x_start..x_end {
                        let color = pattern.color_at(Vec2us::new(x, y)) as usize;
                        samples[color].push(data[y * width + x]);
                    }
                }

                let mut values = [f32::NAN; 3];
                for color in 0..num_colors {
                    if !samples[color].is_empty() {
                        values[color] = median_mut(&mut samples[color]);
                    }
                }
                Ok(DarkTile { values })
            })
            .collect::<Result<_, Error>>()?;

        let missing: [bool; 3] = std::array::from_fn(|color| {
            color < num_colors && tiles.iter().any(|tile| tile.values[color].is_nan())
        });
        for (color, &is_missing) in missing.iter().enumerate().take(num_colors) {
            if !is_missing {
                continue;
            }
            let mut samples = collect_color_samples(data, cfa_type, color as u8);
            if samples.is_empty() {
                continue;
            }
            let fallback = median_mut(&mut samples);
            for tile in &mut tiles {
                if tile.values[color].is_nan() {
                    tile.values[color] = fallback;
                }
            }
        }

        let centers_x = tile_centers(width, tiles_x);
        let centers_y = tile_centers(height, tiles_y);
        Ok(Self {
            tiles: Buffer2::new(tiles_x, tiles_y, tiles),
            x_spans: interpolation_spans(width, &centers_x),
            y_spans: interpolation_spans(height, &centers_y),
        })
    }

    #[inline]
    pub(super) fn at(&self, pos: Vec2us, color: usize) -> f32 {
        let xs = self.x_spans[pos.x];
        let ys = self.y_spans[pos.y];
        let top = lerp(
            self.tiles[(xs.lower, ys.lower)].values[color],
            self.tiles[(xs.upper, ys.lower)].values[color],
            xs.fraction,
        );
        let bottom = lerp(
            self.tiles[(xs.lower, ys.upper)].values[color],
            self.tiles[(xs.upper, ys.upper)].values[color],
            xs.fraction,
        );
        lerp(top, bottom, ys.fraction)
    }
}

fn tile_centers(length: usize, tile_count: usize) -> Vec<f32> {
    (0..tile_count)
        .map(|tile| {
            let start = tile * length / tile_count;
            let end = (tile + 1) * length / tile_count;
            (start + end - 1) as f32 * 0.5
        })
        .collect()
}

fn interpolation_spans(length: usize, centers: &[f32]) -> Vec<InterpolationSpan> {
    if centers.len() == 1 {
        return vec![
            InterpolationSpan {
                lower: 0,
                upper: 0,
                fraction: 0.0,
            };
            length
        ];
    }

    (0..length)
        .map(|position| {
            let position = position as f32;
            let upper = centers
                .partition_point(|&center| center <= position)
                .clamp(1, centers.len() - 1);
            let lower = upper - 1;
            InterpolationSpan {
                lower,
                upper,
                fraction: (position - centers[lower]) / (centers[upper] - centers[lower]),
            }
        })
        .collect()
}

#[inline]
fn lerp(start: f32, end: f32, fraction: f32) -> f32 {
    start + fraction * (end - start)
}
