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

use crate::background_mesh::mesh_axis::MeshAxis;
use crate::concurrency::JobScratchPool;
use crate::io::image::cfa::CfaType;
use crate::math::statistics::median_mut;
use crate::math::vec2us::Vec2us;
use crate::stacking::calibration_masters::defect_map::DARK_BACKGROUND_TILE_SIZE;
use crate::stacking::calibration_masters::defect_map::sampling::collect_color_samples;
use crate::stacking::calibration_masters::error::CalibrationError;
use std::array;

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
        cfa_type: CfaType,
        cancel: &CancelToken,
    ) -> Result<Self, CalibrationError> {
        let width = data.width();
        let height = data.height();
        assert!(
            width > 0 && height > 0,
            "dark background needs non-zero dimensions"
        );
        let columns = MeshAxis::new(width, DARK_BACKGROUND_TILE_SIZE.min(width));
        let rows = MeshAxis::new(height, DARK_BACKGROUND_TILE_SIZE.min(height));
        let (tiles_x, tiles_y) = (columns.count(), rows.count());
        let pattern = cfa_type;
        let num_colors = pattern.num_colors();
        let scratch = JobScratchPool::<[Vec<f32>; 3]>::default();

        let mut tiles: Vec<DarkTile> = (0..tiles_x * tiles_y)
            .into_par_iter()
            .map_init(
                || scratch.acquire(),
                |samples, index| {
                    if cancel.is_cancelled() {
                        return Err(CalibrationError::Cancelled);
                    }

                    let (tx, ty) = (index % tiles_x, index / tiles_x);
                    for plane in samples.iter_mut() {
                        plane.clear();
                    }
                    for y in rows.start(ty)..rows.end(ty) {
                        for x in columns.start(tx)..columns.end(tx) {
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
                },
            )
            .collect::<Result<_, CalibrationError>>()?;

        let missing: [bool; 3] = array::from_fn(|color| {
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

        let centers_x: Vec<f32> = (0..tiles_x).map(|tile| columns.centre(tile)).collect();
        let centers_y: Vec<f32> = (0..tiles_y).map(|tile| rows.centre(tile)).collect();
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
