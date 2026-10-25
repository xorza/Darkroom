//! [`WindowedCentroid`]: the centre of a Gaussian-windowed stamp, iterated to convergence.

use common::Buffer2;
use glam::DVec2;

use crate::star_detection::centroid::covariance::Cov2;
use crate::star_detection::centroid::measure_grid::MeasureGrid;
use crate::star_detection::centroid::stamp::StampAt;
use crate::star_detection::centroid::star_noise::StarNoise;
use crate::star_detection::centroid::windowed_stamp::{Moments, WindowedStamp};

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
pub(super) struct WindowedInputs {
    /// Where the centring starts.
    pub(super) start: DVec2,
    /// The local sky the residual still carries.
    pub(super) offset: f32,
    /// The noise of the star's pixels, its σ held to the frame's floor.
    pub(super) noise: StarNoise,
}

impl WindowedCentroid {
    /// The converged centre from the inputs' start; `None` when the stamp leaves the frame, holds
    /// no positive windowed signal, wanders more than half the stamp radius from the start, or does
    /// not converge within the step budget.
    pub(super) fn measure(
        residual: &Buffer2<f32>,
        grid: &MeasureGrid,
        inputs: WindowedInputs,
    ) -> Option<Self> {
        let start = inputs.start;
        let radius = grid.stamp.radius;
        let window_sq = grid.window_sigma * grid.window_sigma;
        let stamp_at = |pos: DVec2| {
            WindowedStamp::new(
                StampAt {
                    residual,
                    pos,
                    radius,
                },
                window_sq,
            )
        };
        let mut pos = start;
        let mut prev_step: Option<f64> = None;
        for _ in 0..MAX_STEPS {
            let moments = Moments::about(&stamp_at(pos)?, inputs.offset)?;
            let mean = moments.mean();
            let gain = newton_gain(window_sq, moments.about_mean());
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
                || prev_step.is_some_and(|prev| {
                    length < prev && {
                        let contraction = length / prev;
                        contraction / (1.0 - contraction) * length <= TOLERANCE
                    }
                });
            if settled {
                let sigma = centre_sigma(&stamp_at(pos)?, inputs, gain)?;
                return Some(Self { pos, sigma });
            }
            prev_step = Some(length);
        }
        None
    }
}

/// The Newton gain `σ_w²·(σ_w²·I − C)⁻¹` for the windowed covariance `C`, as the symmetric
/// `[g_xx, g_xy, g_yy]`. Where the window's own width does not exceed the profile's windowed
/// spread on both axes — noise in a faint stamp — the step is the plain fixed point.
fn newton_gain(window_sq: f64, covariance: Cov2) -> [f64; 3] {
    let Cov2 { xx, yy, xy } = covariance;
    let (a, b, c) = (window_sq - xx, -xy, window_sq - yy);
    let det = a * c - b * b;
    if a <= 0.0 || det <= 0.0 {
        return [1.0, 0.0, 1.0];
    }
    let scale = window_sq / det;
    [scale * c, -scale * b, scale * a]
}

/// The noise of a centre at `stamp`'s centre: the windowed mean offset `m = Σwv·d / Σwv` varies
/// with pixel `i` as `w_i·(d_i − m)/Σwv`, so `Var(m) = Σ w_i²·(d_i − m)(d_i − m)ᵀ·σ_i² / (Σwv)²`,
/// and the Newton step `gain` carries it as `G·Var(m)·Gᵀ`; `None` when the stamp holds no positive
/// windowed signal.
fn centre_sigma(stamp: &WindowedStamp<'_>, inputs: WindowedInputs, gain: [f64; 3]) -> Option<f64> {
    let moments = Moments::about(stamp, inputs.offset)?;
    let mean = moments.mean();
    let mut spread = [0.0f64; 3];
    for row in stamp.rows() {
        for ((&pixel, &col_weight), &dx) in row
            .pixels
            .iter()
            .zip(stamp.col_weights())
            .zip(stamp.col_offsets())
        {
            let variance = inputs.noise.variance(f64::from(pixel - inputs.offset));
            let w = col_weight * row.weight;
            let (ex, ey) = (dx - mean.x, row.dy - mean.y);
            let scale = w * w * variance;
            spread[0] += scale * ex * ex;
            spread[1] += scale * ex * ey;
            spread[2] += scale * ey * ey;
        }
    }
    let norm = moments.weight() * moments.weight();
    let [vxx, vxy, vyy] = spread.map(|value| value / norm);
    // G·V·Gᵀ for the symmetric G = [[g0, g1], [g1, g2]].
    let [g0, g1, g2] = gain;
    let xx = g0 * (g0 * vxx + g1 * vxy) + g1 * (g0 * vxy + g1 * vyy);
    let yy = g1 * (g1 * vxx + g2 * vxy) + g2 * (g1 * vxy + g2 * vyy);
    Some(f64::midpoint(xx, yy).sqrt())
}
