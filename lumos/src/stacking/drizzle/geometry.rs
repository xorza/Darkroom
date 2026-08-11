//! The geometric primitives drizzle distributes flux with.
//!
//! Two kinds: the area magnification a transform applies ([`AreaMagnification`]), which rescales a
//! drop's contribution, and the exact polygon-to-pixel overlap the square kernel needs ([`sgarea`] /
//! [`boxer`], ported from STScI's `cdrizzlebox.c`). The interpolating kernel itself is
//! `math::lanczos`, shared with `registration::resample`.

use glam::DVec2;

use crate::math::vec2us::Vec2us;
use crate::stacking::registration::transform::{Transform, TransformType};

const SGAREA_DX_MIN: f64 = 1e-14;

/// How much a transform magnifies area — one factor for the frame where the model allows it.
///
/// Only a projective transform magnifies by different amounts in different places. Every other model
/// is linear, so its factor holds everywhere, and measuring it per pixel costs two extra
/// `Transform::apply` calls each — three matrix products where one is needed, in the loop that walks
/// every input pixel of every frame.
///
/// Built from a transform that already carries the drizzle output scale, so there is no separate
/// factor to apply afterwards: `scale` composed into the matrix *is* `scale²` in the determinant.
#[derive(Debug)]
pub(crate) enum AreaMagnification {
    /// `|det(A)|`, the factor a linear model applies at every pixel.
    Uniform(f64),
    /// A projective transform, measured where it is asked for.
    PerPixel(Transform),
}

impl AreaMagnification {
    pub(crate) fn new(to_output: &Transform) -> Self {
        if to_output.transform_type() == TransformType::Homography {
            return Self::PerPixel(*to_output);
        }
        // From the matrix rather than by differencing two transformed points: for a linear model the
        // two are the same quantity, but `a·d − b·c` is the exact expression where the difference of
        // two large coordinates rounds — and it costs one product instead of two transforms.
        let m = to_output.matrix();
        Self::Uniform((m[0] * m[4] - m[1] * m[3]).abs())
    }

    /// The factor at `pixel`, whose transformed position is `center`.
    #[inline]
    pub(crate) fn at(&self, center: DVec2, pixel: Vec2us) -> f64 {
        match self {
            Self::Uniform(factor) => *factor,
            Self::PerPixel(to_output) => local_jacobian(to_output, center, pixel),
        }
    }
}

/// Local Jacobian determinant (area magnification) at `pixel`, by finite differences: transform
/// `center`, `center + dx` and `center + dy`, then `|det([∂out/∂x, ∂out/∂y])|`.
///
/// `to_output` maps input pixels to the output grid, drizzle scale included, so the area it spans is
/// already in output pixels and there is no scale factor to apply here.
///
/// The projective case, where the factor genuinely varies from pixel to pixel.
/// [`AreaMagnification`] is what callers want — it takes this path only when the transform needs it.
#[inline]
pub(crate) fn local_jacobian(to_output: &Transform, center: DVec2, pixel: Vec2us) -> f64 {
    let right = to_output.apply(DVec2::new(pixel.x as f64 + 1.0, pixel.y as f64));
    let down = to_output.apply(DVec2::new(pixel.x as f64, pixel.y as f64 + 1.0));
    let dx = right - center;
    let dy = down - center;
    (dx.x * dy.y - dx.y * dy.x).abs()
}

/// Compute signed area between the segment `from → to` and the x-axis, clipped to the unit square
/// [0,1]×[0,1]. Uses Green's theorem. Port of STScI `sgarea()` from cdrizzlebox.c.
///
/// The sign depends on the direction of traversal (left-to-right = positive).
/// When summed over all 4 edges of a convex quadrilateral (counterclockwise winding),
/// the total gives the overlap area between the quadrilateral and the unit square.
#[inline]
pub(crate) fn sgarea(from: DVec2, to: DVec2) -> f64 {
    let (x1, y1) = (from.x, from.y);
    let (x2, y2) = (to.x, to.y);
    let dx = x2 - x1;
    let dy = y2 - y1;

    if dx.abs() < SGAREA_DX_MIN {
        return 0.0;
    }

    let (sgn_dx, xlo, xhi) = if dx < 0.0 {
        (-1.0, x2, x1)
    } else {
        (1.0, x1, x2)
    };

    if xlo >= 1.0 || xhi <= 0.0 {
        return 0.0;
    }

    let xlo = xlo.max(0.0);
    let xhi = xhi.min(1.0);

    let slope = dy / dx;
    let ylo = y1 + slope * (xlo - x1);
    let yhi = y1 + slope * (xhi - x1);

    if ylo <= 0.0 && yhi <= 0.0 {
        return 0.0;
    }

    if ylo >= 1.0 && yhi >= 1.0 {
        return sgn_dx * (xhi - xlo);
    }

    let det = x1 * y2 - y1 * x2;

    let (xlo, ylo) = if ylo < 0.0 {
        (det / dy, 0.0)
    } else {
        (xlo, ylo)
    };
    let (xhi, yhi) = if yhi < 0.0 {
        (det / dy, 0.0)
    } else {
        (xhi, yhi)
    };

    if ylo <= 1.0 {
        if yhi <= 1.0 {
            return sgn_dx * 0.5 * (xhi - xlo) * (yhi + ylo);
        }
        let xtop = (dx + det) / dy;
        return sgn_dx * (0.5 * (xtop - xlo) * (1.0 + ylo) + xhi - xtop);
    }

    let xtop = (dx + det) / dy;
    sgn_dx * (0.5 * (xhi - xtop) * (1.0 + yhi) + xtop - xlo)
}

/// Compute overlap area between the convex quadrilateral `quad` and a pixel cell.
///
/// Shifts the quadrilateral so that the cell with lower-left `corner` becomes the unit square
/// [0,1]×[0,1], then sums signed areas from each edge via `sgarea()`.
///
/// Port of STScI `boxer()` from cdrizzlebox.c. Output pixels are integer-center (pixel `o`
/// spans `[o - 0.5, o + 0.5]`, matching STScI), so callers pass the cell's lower-left
/// corner `o - 0.5`.
#[inline]
pub(crate) fn boxer(corner: DVec2, quad: &[DVec2; 4]) -> f64 {
    let shifted = quad.map(|vertex| vertex - corner);

    let mut sum = 0.0;
    for i in 0..4 {
        sum += sgarea(shifted[i], shifted[(i + 1) & 3]);
    }
    sum.abs()
}
