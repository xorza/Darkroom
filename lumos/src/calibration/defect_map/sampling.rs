//! Drawing a representative sample of one CFA color.
//!
//! An exact median over every red pixel of a 60 MP sensor is slower than the detection it feeds,
//! so each color is capped at [`MAX_MEDIAN_SAMPLES`]. The sample has to be spread rather than
//! taken from a corner: it is stratified across the color's CFA phases, then across rows, then
//! across columns within each row, with a per-row rotation so a column-aligned defect cannot land
//! in every sampled row.

use arrayvec::ArrayVec;
use common::Buffer2;

use crate::bg_mesh::color_mesh::ColorMesh;
use crate::calibration::defect_map::MAX_MEDIAN_SAMPLES;
use crate::image::cfa::cfa_type::CfaType;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

#[derive(Debug, Clone, Copy)]
struct CfaSamplePhase {
    x_offset: usize,
    y_offset: usize,
    cols: usize,
    rows: usize,
    population: usize,
    sample_count: usize,
}

/// A master's samples over its color mesh, read color by color of its `cfa_type`.
#[derive(Debug, Clone, Copy)]
pub(super) struct ColorResiduals<'a> {
    pub(super) data: &'a Buffer2<f32>,
    pub(super) cfa_type: CfaType,
    pub(super) bg: &'a ColorMesh,
}

impl ColorResiduals<'_> {
    /// The residuals of `target_color`'s stratified sample.
    pub(super) fn samples(self, target_color: usize) -> Vec<f32> {
        let Self { data, cfa_type, bg } = self;
        let size = Size2us::from(data);
        color_sample_idxs(size, cfa_type, target_color)
            .into_iter()
            .map(|idx| data[idx] - bg.at(target_color, size.point_of(idx)).sky)
            .collect()
    }
}

/// The sorted flat indices of `target_color`'s stratified sample of a `size` frame: every
/// photosite of the color, or [`MAX_MEDIAN_SAMPLES`] of them spread across its phases, rows and
/// columns.
pub(super) fn color_sample_idxs(
    size: Size2us,
    cfa_type: CfaType,
    target_color: usize,
) -> Vec<usize> {
    debug_assert!(
        size.width > 0 && size.height > 0,
        "color sampling needs non-zero dimensions"
    );

    let period = cfa_type.period();

    let mut phases =
        ArrayVec::<CfaSamplePhase, { CfaType::MAX_PERIOD * CfaType::MAX_PERIOD }>::new();
    for y_offset in 0..period.min(size.height) {
        for x_offset in 0..period.min(size.width) {
            if cfa_type.slot_at(Vec2us::new(x_offset, y_offset)) != target_color {
                continue;
            }
            let cols = (size.width - 1 - x_offset) / period + 1;
            let rows = (size.height - 1 - y_offset) / period + 1;
            phases.push(CfaSamplePhase {
                x_offset,
                y_offset,
                cols,
                rows,
                population: cols * rows,
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

    let mut idxs = Vec::with_capacity(target_sample_count);
    for phase in phases {
        if phase.sample_count == phase.population {
            for row in 0..phase.rows {
                let y = phase.y_offset + row * period;
                for col in 0..phase.cols {
                    let x = phase.x_offset + col * period;
                    idxs.push(size.idx_of(Vec2us::new(x, y)));
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
            let rotation = (sample_row + phase_rotation) % phase.cols;

            for sample_col in 0..row_sample_count {
                let col = (stratified_center(sample_col, row_sample_count, phase.cols) + rotation)
                    % phase.cols;
                let x = phase.x_offset + col * period;
                idxs.push(size.idx_of(Vec2us::new(x, y)));
            }
        }
    }

    idxs.sort_unstable();
    debug_assert_eq!(idxs.len(), target_sample_count);
    debug_assert!(idxs.windows(2).all(|pair| pair[0] < pair[1]));
    idxs
}

const fn scaled_partition(part: usize, part_count: usize, length: usize) -> usize {
    (part as u128 * length as u128 / part_count as u128) as usize
}

const fn stratified_center(part: usize, part_count: usize, length: usize) -> usize {
    ((2 * part as u128 + 1) * length as u128 / (2 * part_count as u128)) as usize
}

#[cfg(test)]
pub(crate) mod internals {
    use common::Buffer2;

    use crate::calibration::defect_map::sampling;
    use crate::image::cfa::cfa_type::CfaType;
    use crate::math::size2us::Size2us;

    /// The values of [`sampling::color_sample_idxs`]' stratified sample of one color.
    pub(crate) fn color_samples(
        data: &Buffer2<f32>,
        cfa_type: CfaType,
        target_color: usize,
    ) -> Vec<f32> {
        sampling::color_sample_idxs(
            Size2us::new(data.width(), data.height()),
            cfa_type,
            target_color,
        )
        .into_iter()
        .map(|idx| data[idx])
        .collect()
    }
}
