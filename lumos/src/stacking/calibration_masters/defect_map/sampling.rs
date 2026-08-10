//! Drawing a representative sample of one CFA colour.
//!
//! An exact median over every red pixel of a 60 MP sensor is slower than the detection it feeds,
//! so each colour is capped at [`MAX_MEDIAN_SAMPLES`]. The sample has to be spread rather than
//! taken from a corner: it is stratified across the colour's CFA phases, then across rows, then
//! across columns within each row, with a per-row rotation so a column-aligned defect cannot land
//! in every sampled row.

use arrayvec::ArrayVec;
use imaginarium::Buffer2;

use crate::io::image::cfa::CfaType;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::stacking::calibration_masters::defect_map::dark_background::DarkBackground;
use crate::stacking::calibration_masters::defect_map::{MAX_MEDIAN_SAMPLES, cfa_color_at};

#[derive(Debug, Clone, Copy)]
struct CfaSamplePhase {
    x_offset: usize,
    y_offset: usize,
    columns: usize,
    rows: usize,
    population: usize,
    sample_count: usize,
}

pub(super) fn collect_color_residual_samples(
    data: &Buffer2<f32>,
    cfa_type: Option<&CfaType>,
    target_color: u8,
    background: &DarkBackground,
) -> Vec<f32> {
    let size = Size2us::new(data.width(), data.height());
    collect_color_sample_indices(size, cfa_type, target_color)
        .into_iter()
        .map(|index| data[index] - background.at(size.point_of(index), target_color as usize))
        .collect()
}

/// Collect pixel samples for a specific CFA color channel.
///
/// Large channels are stratified across CFA phases, rows, and columns.
pub(super) fn collect_color_samples(
    data: &Buffer2<f32>,
    cfa_type: Option<&CfaType>,
    target_color: u8,
) -> Vec<f32> {
    collect_color_sample_indices(
        Size2us::new(data.width(), data.height()),
        cfa_type,
        target_color,
    )
    .into_iter()
    .map(|index| data[index])
    .collect()
}

pub(super) fn collect_color_sample_indices(
    size: Size2us,
    cfa_type: Option<&CfaType>,
    target_color: u8,
) -> Vec<usize> {
    assert!(
        size.width > 0 && size.height > 0,
        "color sampling needs non-zero dimensions"
    );

    let period = match cfa_type {
        None | Some(CfaType::Mono) => 1,
        Some(CfaType::Bayer(_)) => 2,
        Some(CfaType::XTrans(_)) => 6,
    };

    let mut phases = ArrayVec::<CfaSamplePhase, 36>::new();
    for y_offset in 0..period.min(size.height) {
        for x_offset in 0..period.min(size.width) {
            if cfa_color_at(cfa_type, Vec2us::new(x_offset, y_offset)) != target_color {
                continue;
            }
            let columns = (size.width - 1 - x_offset) / period + 1;
            let rows = (size.height - 1 - y_offset) / period + 1;
            phases.push(CfaSamplePhase {
                x_offset,
                y_offset,
                columns,
                rows,
                population: columns * rows,
                sample_count: 0,
            });
        }
    }

    let population: usize = phases.iter().map(|phase| phase.population).sum();
    if population == 0 {
        return Vec::new();
    }
    let target_sample_count = population.min(MAX_MEDIAN_SAMPLES);
    let mut cumulative_population = 0;
    let mut allocated = 0;
    for phase in &mut phases {
        cumulative_population += phase.population;
        let next_allocated =
            scaled_partition(cumulative_population, population, target_sample_count);
        phase.sample_count = next_allocated - allocated;
        allocated = next_allocated;
    }

    let mut indices = Vec::with_capacity(target_sample_count);
    for phase in phases {
        if phase.sample_count == phase.population {
            for row in 0..phase.rows {
                let y = phase.y_offset + row * period;
                for column in 0..phase.columns {
                    let x = phase.x_offset + column * period;
                    indices.push(size.index_of(Vec2us::new(x, y)));
                }
            }
            continue;
        }

        let sampled_rows = phase.rows.min(phase.sample_count);
        let phase_rotation = phase.y_offset * period + phase.x_offset;
        for sample_row in 0..sampled_rows {
            let row = stratified_center(sample_row, sampled_rows, phase.rows);
            let y = phase.y_offset + row * period;
            let row_sample_start = scaled_partition(sample_row, sampled_rows, phase.sample_count);
            let row_sample_end = scaled_partition(sample_row + 1, sampled_rows, phase.sample_count);
            let row_sample_count = row_sample_end - row_sample_start;
            let rotation = (sample_row + phase_rotation) % phase.columns;

            for sample_column in 0..row_sample_count {
                let column = (stratified_center(sample_column, row_sample_count, phase.columns)
                    + rotation)
                    % phase.columns;
                let x = phase.x_offset + column * period;
                indices.push(size.index_of(Vec2us::new(x, y)));
            }
        }
    }

    indices.sort_unstable();
    debug_assert_eq!(indices.len(), target_sample_count);
    debug_assert!(indices.windows(2).all(|pair| pair[0] < pair[1]));
    indices
}

fn scaled_partition(part: usize, part_count: usize, length: usize) -> usize {
    (part as u128 * length as u128 / part_count as u128) as usize
}

fn stratified_center(part: usize, part_count: usize, length: usize) -> usize {
    ((2 * part as u128 + 1) * length as u128 / (2 * part_count as u128)) as usize
}
