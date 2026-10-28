//! [`WindowedStamp`]: a stamp under a circular Gaussian window, and the moments it weighs.

use common::Buffer2;
use glam::{DVec2, USizeVec2};

use crate::star_detection::centroid::covariance::Cov2;
use crate::star_detection::centroid::stamp::{MAX_STAMP_SIZE, StampAt};

/// A stamp about a centre under a circular Gaussian window of variance σ_w², with the window's
/// per-axis weights and offsets: the window factors as `exp(−dx²/2σ_w²)·exp(−dy²/2σ_w²)`, so a
/// stamp takes `2(2r + 1)` exponentials instead of `(2r + 1)²`.
#[derive(Debug)]
pub(super) struct WindowedStamp<'a> {
    residual: &'a Buffer2<f32>,
    /// The stamp's top-left pixel.
    origin: USizeVec2,
    size: usize,
    pos: DVec2,
    inv_two_window: f64,
    col_weights: [f64; MAX_STAMP_SIZE],
    col_offsets: [f64; MAX_STAMP_SIZE],
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
        let origin = centre - USizeVec2::splat(radius);
        let mut col_weights = [0.0; MAX_STAMP_SIZE];
        let mut col_offsets = [0.0; MAX_STAMP_SIZE];
        for col in 0..size {
            let dx = (origin.x + col) as f64 - pos.x;
            col_offsets[col] = dx;
            col_weights[col] = (-dx * dx * inv_two_window).exp();
        }
        Some(Self {
            residual,
            origin,
            size,
            pos,
            inv_two_window,
            col_weights,
            col_offsets,
        })
    }

    /// Each row of the stamp, top to bottom.
    pub(super) fn rows(&self) -> impl Iterator<Item = WindowedRow<'a>> + '_ {
        (self.origin.y..self.origin.y + self.size).map(|y| {
            let dy = y as f64 - self.pos.y;
            WindowedRow {
                pixels: &self.residual.row(y)[self.origin.x..self.origin.x + self.size],
                weight: (-dy * dy * self.inv_two_window).exp(),
                dy,
            }
        })
    }

    /// Each column's window weight along x, left to right.
    pub(super) fn col_weights(&self) -> &[f64] {
        &self.col_weights[..self.size]
    }

    /// Each column's offset from the centre, left to right.
    pub(super) fn col_offsets(&self) -> &[f64] {
        &self.col_offsets[..self.size]
    }
}

impl Moments {
    /// The windowed moments of the signed signal less `offset` over `stamp`; `None` when the
    /// stamp holds no positive windowed signal.
    pub(super) fn about(stamp: &WindowedStamp<'_>, offset: f32) -> Option<Self> {
        let mut moments = Self::default();
        for row in stamp.rows() {
            let mut row_sum = Self::default();
            for ((&value, &col_weight), &dx) in row
                .pixels
                .iter()
                .zip(stamp.col_weights())
                .zip(stamp.col_offsets())
            {
                let wv = col_weight * f64::from(value - offset);
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
