//! [`WindowedCentroid`]: the centre of a Gaussian-windowed stamp, iterated to convergence.

use glam::DVec2;
use imaginarium::Buffer2;

use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::star_detection::centroid::measure_grid::MeasureGrid;
use crate::star_detection::centroid::{MAX_STAMP_SIZE, stamp_centre};
use crate::star_detection::config::measurement_config::NoiseModel;

/// The error left in a centre at which the iteration stops, in pixels: a hundredth of what the
/// brightest stars' noise allows, about 1e-3 px.
const TOLERANCE: f64 = 1e-5;

/// The steps one centre may take; a profile the iteration contracts on reaches [`TOLERANCE`] in a
/// handful.
const MAX_STEPS: usize = 30;

/// A star's centre as Gaussian-windowed first moments find it, iterated to convergence, with the
/// position σ the pixel noise propagates into it.
///
/// Each step weights the stamp's signed signal above the local sky by a circular Gaussian window
/// of σ_w about the current centre, and moves the centre by the adaptive-moments Newton step
/// `σ_w²·(σ_w²·I − C)⁻¹·m`, with `m` the windowed mean offset and `C` the windowed covariance about
/// it. Under the window a Gaussian star of σ_s reads `C = σ_w²σ_s²/(σ_w² + σ_s²)`, so the step
/// lands on its centre in one iteration whatever σ_w; SExtractor's XWIN takes σ_w = σ_s and doubles
/// `m`, which is this step at that width. For any other profile the steps contract by some
/// `c < 1`, and the iteration stops when the error left, `c/(1 − c)·‖Δ‖` with `c` the ratio of the
/// last two steps, is under [`TOLERANCE`]. A plain fixed-point step contracts by
/// `σ_s²/(σ_s² + σ_w²)`, 0.6 at the matched window, and stopped on its step size understates the
/// error left by `c/(1 − c)`.
///
/// The signal is signed, as SExtractor's: clipping it at zero lifts the sky noise into a pedestal
/// that slows the contraction and biases faint stars.
#[derive(Debug, Clone, Copy)]
pub(super) struct WindowedCentroid {
    pub(super) pos: DVec2,
    /// `√((σ_x² + σ_y²)/2)` of the centre, from the pixel noise.
    pub(super) sigma: f64,
}

/// What a windowed centroid reads of a stamp besides its pixels.
#[derive(Debug, Clone, Copy)]
pub(super) struct WindowedInputs<'a> {
    /// The local sky the residual still carries.
    pub(super) offset: f32,
    /// The sky's σ per pixel, held to the frame's floor.
    pub(super) sky_sigma: f32,
    pub(super) noise_model: Option<&'a NoiseModel>,
}

/// The windowed moments about one centre.
#[derive(Debug, Default)]
struct Moments {
    weight: f64,
    x: f64,
    y: f64,
    xx: f64,
    yy: f64,
    xy: f64,
}

impl WindowedCentroid {
    /// The converged centre from `start`; `None` when the stamp leaves the frame, holds no positive
    /// windowed signal, wanders more than half the stamp radius from `start`, or does not converge
    /// within the step budget.
    pub(super) fn measure(
        residual: &Buffer2<f32>,
        start: DVec2,
        grid: &MeasureGrid,
        inputs: WindowedInputs<'_>,
    ) -> Option<Self> {
        let radius = grid.stamp.radius;
        let window_sq = grid.window_sigma * grid.window_sigma;
        let mut pos = start;
        let mut previous_step: Option<f64> = None;
        for _ in 0..MAX_STEPS {
            let moments = Moments::about(residual, pos, grid, inputs.offset)?;
            let mean = DVec2::new(moments.x, moments.y) / moments.weight;
            let covariance = [
                moments.xx / moments.weight - mean.x * mean.x,
                moments.xy / moments.weight - mean.x * mean.y,
                moments.yy / moments.weight - mean.y * mean.y,
            ];
            let gain = newton_gain(window_sq, covariance);
            let step = DVec2::new(
                gain[0] * mean.x + gain[1] * mean.y,
                gain[1] * mean.x + gain[2] * mean.y,
            );
            pos += step;
            if !pos.is_finite() || (pos - start).abs().max_element() > radius as f64 / 2.0 {
                return None;
            }
            let length = step.length();
            // A step at the arithmetic's own noise, a thousand ulps of the coordinate, is no
            // step: a point-symmetric stamp cancels its mean only to rounding.
            let rounding = 1e3 * f64::EPSILON * pos.abs().max_element().max(1.0);
            let settled = length <= rounding
                || previous_step.is_some_and(|previous| {
                    length < previous && {
                        let contraction = length / previous;
                        contraction / (1.0 - contraction) * length <= TOLERANCE
                    }
                });
            if settled {
                let sigma = Self::sigma_at(residual, pos, grid, inputs, gain)?;
                return Some(Self { pos, sigma });
            }
            previous_step = Some(length);
        }
        None
    }

    /// The noise of a centre at `pos`: the windowed mean offset `m = Σwv·d / Σwv` varies with
    /// pixel `i` as `w_i·(d_i − m)/Σwv`, so `Var(m) = Σ w_i²·(d_i − m)(d_i − m)ᵀ·σ_i² / (Σwv)²`, and
    /// the Newton step carries it as `G·Var(m)·Gᵀ`.
    fn sigma_at(
        residual: &Buffer2<f32>,
        pos: DVec2,
        grid: &MeasureGrid,
        inputs: WindowedInputs<'_>,
        gain: [f64; 3],
    ) -> Option<f64> {
        let radius = grid.stamp.radius;
        let centre = stamp_centre(
            pos,
            Size2us::new(residual.width(), residual.height()),
            radius,
        )?;
        let inv_two_window = 1.0 / (2.0 * grid.window_sigma * grid.window_sigma);
        let sky_variance = f64::from(inputs.sky_sigma) * f64::from(inputs.sky_sigma);
        let mut weight = 0.0;
        let mut mean = DVec2::ZERO;
        let mut spread = [0.0f64; 3];
        let stamp = Stamp::around(centre, radius, pos, inv_two_window);
        for (y, row_weight, dy) in stamp.rows() {
            let row = residual.row(y);
            for (column, (&column_weight, &dx)) in stamp.columns() {
                let value = f64::from(row[stamp.x0 + column] - inputs.offset);
                let w = column_weight * row_weight;
                weight += w * value;
                mean += w * value * DVec2::new(dx, dy);
            }
        }
        if weight <= 0.0 {
            return None;
        }
        mean /= weight;
        for (y, row_weight, dy) in stamp.rows() {
            let row = residual.row(y);
            for (column, (&column_weight, &dx)) in stamp.columns() {
                let value = f64::from(row[stamp.x0 + column] - inputs.offset);
                let variance = inputs.noise_model.map_or(sky_variance, |model| {
                    model.variance_normalized(value.max(0.0), f64::from(inputs.sky_sigma), 1)
                });
                let w = column_weight * row_weight;
                let (ex, ey) = (dx - mean.x, dy - mean.y);
                let scale = w * w * variance;
                spread[0] += scale * ex * ex;
                spread[1] += scale * ex * ey;
                spread[2] += scale * ey * ey;
            }
        }
        let norm = weight * weight;
        let [vxx, vxy, vyy] = spread.map(|value| value / norm);
        // G·V·Gᵀ for the symmetric G = [[g0, g1], [g1, g2]].
        let [g0, g1, g2] = gain;
        let xx = g0 * (g0 * vxx + g1 * vxy) + g1 * (g0 * vxy + g1 * vyy);
        let yy = g1 * (g1 * vxx + g2 * vxy) + g2 * (g1 * vxy + g2 * vyy);
        Some(f64::midpoint(xx, yy).sqrt())
    }
}

/// The Newton gain `σ_w²·(σ_w²·I − C)⁻¹` for the windowed covariance `C = [xx, xy, yy]`, as the
/// symmetric `[g_xx, g_xy, g_yy]`. Where the window's own width does not exceed the profile's
/// windowed spread on both axes — noise in a faint stamp — the step is the plain fixed point.
fn newton_gain(window_sq: f64, covariance: [f64; 3]) -> [f64; 3] {
    let [xx, xy, yy] = covariance;
    let (a, b, c) = (window_sq - xx, -xy, window_sq - yy);
    let det = a * c - b * b;
    if a <= 0.0 || det <= 0.0 {
        return [1.0, 0.0, 1.0];
    }
    let scale = window_sq / det;
    [scale * c, -scale * b, scale * a]
}

/// A stamp about a centre, with its window's per-axis weights and offsets.
#[derive(Debug)]
struct Stamp {
    x0: usize,
    y0: usize,
    size: usize,
    pos: DVec2,
    inv_two_window: f64,
    column_weights: [f64; MAX_STAMP_SIZE],
    column_offsets: [f64; MAX_STAMP_SIZE],
}

impl Stamp {
    fn around(centre: Vec2us, radius: usize, pos: DVec2, inv_two_window: f64) -> Self {
        let size = 2 * radius + 1;
        let (x0, y0) = (centre.x - radius, centre.y - radius);
        let mut column_weights = [0.0; MAX_STAMP_SIZE];
        let mut column_offsets = [0.0; MAX_STAMP_SIZE];
        for column in 0..size {
            let dx = (x0 + column) as f64 - pos.x;
            column_offsets[column] = dx;
            column_weights[column] = (-dx * dx * inv_two_window).exp();
        }
        Self {
            x0,
            y0,
            size,
            pos,
            inv_two_window,
            column_weights,
            column_offsets,
        }
    }

    /// Each row: its image y, its window weight and its offset from the centre.
    fn rows(&self) -> impl Iterator<Item = (usize, f64, f64)> + '_ {
        (self.y0..self.y0 + self.size).map(|y| {
            let dy = y as f64 - self.pos.y;
            (y, (-dy * dy * self.inv_two_window).exp(), dy)
        })
    }

    /// Each column of the stamp, with its window weight and offset.
    fn columns(&self) -> impl Iterator<Item = (usize, (&f64, &f64))> + '_ {
        self.column_weights[..self.size]
            .iter()
            .zip(&self.column_offsets[..self.size])
            .enumerate()
    }
}

impl Moments {
    /// The windowed moments of the signed signal less `offset` about `pos`, offsets measured from
    /// `pos`; `None` when the stamp leaves the frame or holds no positive windowed signal.
    fn about(residual: &Buffer2<f32>, pos: DVec2, grid: &MeasureGrid, offset: f32) -> Option<Self> {
        let radius = grid.stamp.radius;
        let centre = stamp_centre(
            pos,
            Size2us::new(residual.width(), residual.height()),
            radius,
        )?;
        let inv_two_window = 1.0 / (2.0 * grid.window_sigma * grid.window_sigma);
        let stamp = Stamp::around(centre, radius, pos, inv_two_window);
        let mut moments = Self::default();
        for (y, row_weight, dy) in stamp.rows() {
            let row = &residual.row(y)[stamp.x0..stamp.x0 + stamp.size];
            let mut row_sum = Self::default();
            for ((&value, &column_weight), &dx) in row
                .iter()
                .zip(&stamp.column_weights[..stamp.size])
                .zip(&stamp.column_offsets[..stamp.size])
            {
                let wv = column_weight * f64::from(value - offset);
                row_sum.weight += wv;
                row_sum.x += wv * dx;
                row_sum.xx += wv * dx * dx;
            }
            moments.weight += row_weight * row_sum.weight;
            moments.x += row_weight * row_sum.x;
            moments.y += row_weight * row_sum.weight * dy;
            moments.xx += row_weight * row_sum.xx;
            moments.yy += row_weight * row_sum.weight * dy * dy;
            moments.xy += row_weight * row_sum.x * dy;
        }
        (moments.weight > 0.0).then_some(moments)
    }
}
