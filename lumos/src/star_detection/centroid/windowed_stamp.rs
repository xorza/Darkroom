//! [`WindowedStamp`]: a stamp under a circular Gaussian window, and the moments it weighs.

use glam::DVec2;
use imaginarium::Buffer2;

use crate::star_detection::centroid::covariance::Cov2;
use crate::star_detection::centroid::stamp::{MAX_STAMP_SIZE, StampAt};

/// A stamp about a centre under a circular Gaussian window of variance σ_w², with the window's
/// per-axis weights and offsets: the window factors as `exp(−dx²/2σ_w²)·exp(−dy²/2σ_w²)`, so a
/// stamp takes `2(2r + 1)` exponentials instead of `(2r + 1)²`.
#[derive(Debug)]
pub(super) struct WindowedStamp<'a> {
    residual: &'a Buffer2<f32>,
    x0: usize,
    y0: usize,
    size: usize,
    pos: DVec2,
    inv_two_window: f64,
    column_weights: [f64; MAX_STAMP_SIZE],
    column_offsets: [f64; MAX_STAMP_SIZE],
}

/// One row of a [`WindowedStamp`]: its pixels, their window weight along y and offset from the
/// centre.
#[derive(Debug, Clone, Copy)]
pub(super) struct WindowedRow<'a> {
    pub(super) pixels: &'a [f32],
    pub(super) weight: f64,
    pub(super) dy: f64,
}

/// The windowed moments of the signal about a stamp's centre: `Σw·v` and its first and second
/// moments in the offsets from the centre.
#[derive(Debug, Default)]
pub(super) struct Moments {
    weight: f64,
    x: f64,
    y: f64,
    xx: f64,
    yy: f64,
    xy: f64,
}

impl<'a> WindowedStamp<'a> {
    /// `stamp` under a window of variance `window_sq` about its `pos`; `None` when it leaves the
    /// frame.
    pub(super) fn new(stamp: StampAt<'a>, window_sq: f64) -> Option<Self> {
        let StampAt {
            residual,
            pos,
            radius,
        } = stamp;
        let centre = stamp.centre()?;
        let inv_two_window = 1.0 / (2.0 * window_sq);
        let size = 2 * radius + 1;
        let (x0, y0) = (centre.x - radius, centre.y - radius);
        let mut column_weights = [0.0; MAX_STAMP_SIZE];
        let mut column_offsets = [0.0; MAX_STAMP_SIZE];
        for column in 0..size {
            let dx = (x0 + column) as f64 - pos.x;
            column_offsets[column] = dx;
            column_weights[column] = (-dx * dx * inv_two_window).exp();
        }
        Some(Self {
            residual,
            x0,
            y0,
            size,
            pos,
            inv_two_window,
            column_weights,
            column_offsets,
        })
    }

    /// Each row of the stamp, top to bottom.
    pub(super) fn rows(&self) -> impl Iterator<Item = WindowedRow<'a>> + '_ {
        (self.y0..self.y0 + self.size).map(|y| {
            let dy = y as f64 - self.pos.y;
            WindowedRow {
                pixels: &self.residual.row(y)[self.x0..self.x0 + self.size],
                weight: (-dy * dy * self.inv_two_window).exp(),
                dy,
            }
        })
    }

    /// Each column's window weight along x, left to right.
    pub(super) fn column_weights(&self) -> &[f64] {
        &self.column_weights[..self.size]
    }

    /// Each column's offset from the centre, left to right.
    pub(super) fn column_offsets(&self) -> &[f64] {
        &self.column_offsets[..self.size]
    }
}

impl Moments {
    /// The windowed moments of the signed signal less `offset` over `stamp`; `None` when the
    /// stamp holds no positive windowed signal.
    pub(super) fn about(stamp: &WindowedStamp<'_>, offset: f32) -> Option<Self> {
        let mut moments = Self::default();
        for row in stamp.rows() {
            let mut row_sum = Self::default();
            for ((&value, &column_weight), &dx) in row
                .pixels
                .iter()
                .zip(stamp.column_weights())
                .zip(stamp.column_offsets())
            {
                let wv = column_weight * f64::from(value - offset);
                row_sum.weight += wv;
                row_sum.x += wv * dx;
                row_sum.xx += wv * dx * dx;
            }
            let (weight, dy) = (row.weight, row.dy);
            moments.weight += weight * row_sum.weight;
            moments.x += weight * row_sum.x;
            moments.y += weight * row_sum.weight * dy;
            moments.xx += weight * row_sum.xx;
            moments.yy += weight * row_sum.weight * dy * dy;
            moments.xy += weight * row_sum.x * dy;
        }
        (moments.weight > 0.0).then_some(moments)
    }

    /// `Σw·v`, positive.
    pub(super) const fn weight(&self) -> f64 {
        self.weight
    }

    /// The windowed mean offset from the centre.
    pub(super) fn mean(&self) -> DVec2 {
        DVec2::new(self.x, self.y) / self.weight
    }

    /// The windowed second moments about the stamp's centre.
    pub(super) const fn about_centre(&self) -> Cov2 {
        Cov2 {
            xx: self.xx / self.weight,
            yy: self.yy / self.weight,
            xy: self.xy / self.weight,
        }
    }

    /// The windowed covariance about the windowed mean.
    pub(super) fn about_mean(&self) -> Cov2 {
        let mean = self.mean();
        let centre = self.about_centre();
        Cov2 {
            xx: centre.xx - mean.x * mean.x,
            yy: centre.yy - mean.y * mean.y,
            xy: centre.xy - mean.x * mean.y,
        }
    }
}
