//! Geometric support and interpolation-confidence maps.

use rayon::prelude::*;

use crate::stacking::registration::config::InterpolationMethod;
use crate::stacking::registration::resample::{kernel, row};
use crate::stacking::registration::transform::WarpTransform;
use common::Vec2us;
use glam::Vec2;
use imaginarium::Buffer2;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, Default)]
struct AxisWeightStats {
    magnitude: f32,
    in_signed: f32,
    in_magnitude: f32,
    in_square: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct SampleQuality {
    coverage: f32,
    confidence: f32,
}

#[derive(Debug)]
pub(crate) struct Maps {
    pub(crate) coverage: Buffer2<f32>,
    pub(crate) confidence: Buffer2<f32>,
}

fn axis_weight_stats(start: i32, weights: &[f32], length: usize) -> AxisWeightStats {
    let mut stats = AxisWeightStats::default();
    for (i, &weight) in weights.iter().enumerate() {
        let magnitude = weight.abs();
        let square = weight * weight;
        stats.magnitude += magnitude;
        let coordinate = start + i as i32;
        if coordinate >= 0 && (coordinate as usize) < length {
            stats.in_signed += weight;
            stats.in_magnitude += magnitude;
            stats.in_square += square;
        }
    }
    stats
}

fn separable_coverage(x: AxisWeightStats, y: AxisWeightStats) -> f32 {
    let total = x.magnitude * y.magnitude;
    if total <= f32::EPSILON {
        0.0
    } else {
        ((x.in_magnitude * y.in_magnitude) / total).clamp(0.0, 1.0)
    }
}

fn separable_confidence(x: AxisWeightStats, y: AxisWeightStats) -> f32 {
    let normalization = x.in_signed * y.in_signed;
    let square = x.in_square * y.in_square;
    if normalization.abs() <= 1e-10 || square <= f32::EPSILON {
        0.0
    } else {
        normalization * normalization / square
    }
}

fn bilinear_quality(pos: Vec2, dims: Vec2us) -> SampleQuality {
    let (sx, sy) = (pos.x, pos.y);
    let x0 = sx.floor() as i32;
    let y0 = sy.floor() as i32;
    let fx = sx - x0 as f32;
    let fy = sy - y0 as f32;
    let x = axis_weight_stats(x0, &[1.0 - fx, fx], dims.x);
    let y = axis_weight_stats(y0, &[1.0 - fy, fy], dims.y);
    SampleQuality {
        coverage: separable_coverage(x, y),
        confidence: separable_confidence(x, y),
    }
}

fn quality_at(pos: Vec2, dims: Vec2us, method: InterpolationMethod) -> SampleQuality {
    if !kernel::source_footprint_contains(pos, dims) {
        return SampleQuality::default();
    }
    let (sx, sy) = (pos.x, pos.y);
    match method {
        InterpolationMethod::Nearest => SampleQuality {
            coverage: 1.0,
            confidence: 1.0,
        },
        InterpolationMethod::Bilinear => bilinear_quality(pos, dims),
        InterpolationMethod::Bicubic => {
            let x0 = sx.floor() as i32;
            let y0 = sy.floor() as i32;
            let fx = sx - x0 as f32;
            let fy = sy - y0 as f32;
            let wx = kernel::bicubic_weights(fx);
            let wy = kernel::bicubic_weights(fy);
            let x = axis_weight_stats(x0 - 1, &wx, dims.x);
            let y = axis_weight_stats(y0 - 1, &wy, dims.y);
            SampleQuality {
                coverage: separable_coverage(x, y),
                confidence: separable_confidence(x, y),
            }
        }
        InterpolationMethod::Lanczos2
        | InterpolationMethod::Lanczos3
        | InterpolationMethod::Lanczos4 => {
            let a = method.lanczos_param().unwrap();
            let lut = kernel::get_lanczos_lut(a);
            let x0 = sx.floor() as i32;
            let y0 = sy.floor() as i32;
            let fx = sx - x0 as f32;
            let fy = sy - y0 as f32;
            let ai = a as i32;
            let size = 2 * a;
            let mut wx = [0.0f32; 8];
            let mut wy = [0.0f32; 8];
            for i in 0..size {
                wx[i] = lut.lookup(fx - (i as i32 - ai + 1) as f32);
                wy[i] = lut.lookup(fy - (i as i32 - ai + 1) as f32);
            }
            let start_x = x0 - ai + 1;
            let start_y = y0 - ai + 1;
            let x = axis_weight_stats(start_x, &wx[..size], dims.x);
            let y = axis_weight_stats(start_y, &wy[..size], dims.y);
            let coverage = separable_coverage(x, y);
            let fully_supported = start_x >= 0
                && start_y >= 0
                && start_x + size as i32 <= dims.x as i32
                && start_y + size as i32 <= dims.y as i32;
            let confidence = if coverage == 0.0 {
                0.0
            } else if fully_supported {
                separable_confidence(x, y)
            } else {
                bilinear_quality(kernel::clamp_to_pixel_centers(pos, dims), dims).confidence
            };
            SampleQuality {
                coverage,
                confidence,
            }
        }
    }
}

pub(crate) fn maps(dims: Vec2us, transform: &WarpTransform, method: InterpolationMethod) -> Maps {
    let mut coverage = Buffer2::new_default(dims.x, dims.y);
    let mut confidence = Buffer2::new_default(dims.x, dims.y);
    coverage
        .pixels_mut()
        .par_chunks_mut(dims.x)
        .zip(confidence.pixels_mut().par_chunks_mut(dims.x))
        .enumerate()
        .for_each(|(y, (coverage_row, confidence_row))| {
            row::for_each_source_position(y, transform, dims.x, |x, pos| {
                let quality =
                    pos.map_or_else(SampleQuality::default, |pos| quality_at(pos, dims, method));
                coverage_row[x] = quality.coverage;
                confidence_row[x] = quality.confidence;
            });
        });

    Maps {
        coverage,
        confidence,
    }
}
