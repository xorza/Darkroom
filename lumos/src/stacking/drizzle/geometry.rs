use std::f32::consts::PI;

use glam::DVec2;

use crate::stacking::registration::transform::Transform;

const SINC_ZERO_THRESHOLD: f32 = 1e-6;
const SGAREA_DX_MIN: f64 = 1e-14;

/// Compute local Jacobian determinant (area magnification) at pixel `(ix, iy)`.
///
/// Uses finite differences: transforms center, center+dx, center+dy through the
/// transform and computes |det([∂out/∂x, ∂out/∂y])| * scale².
///
/// For affine transforms this is constant (= det(M) * scale²).
/// For homographies it varies spatially.
#[inline]
pub(crate) fn local_jacobian(
    transform: &Transform,
    center: DVec2,
    ix: usize,
    iy: usize,
    scale: f64,
) -> f64 {
    let right = transform.apply(DVec2::new(ix as f64 + 1.0, iy as f64));
    let down = transform.apply(DVec2::new(ix as f64, iy as f64 + 1.0));
    let dx = right - center;
    let dy = down - center;
    (dx.x * dy.y - dx.y * dy.x).abs() * scale * scale
}

/// Lanczos kernel function.
#[inline]
pub(crate) fn lanczos_kernel(x: f32, a: f32) -> f32 {
    if x.abs() < SINC_ZERO_THRESHOLD {
        return 1.0;
    }
    if x.abs() >= a {
        return 0.0;
    }
    let pi_x = PI * x;
    let pi_x_a = pi_x / a;
    (pi_x.sin() / pi_x) * (pi_x_a.sin() / pi_x_a)
}

/// Compute signed area between a line segment and the x-axis, clipped to the unit square
/// [0,1]×[0,1]. Uses Green's theorem. Port of STScI `sgarea()` from cdrizzlebox.c.
///
/// The sign depends on the direction of traversal (left-to-right = positive).
/// When summed over all 4 edges of a convex quadrilateral (counterclockwise winding),
/// the total gives the overlap area between the quadrilateral and the unit square.
#[inline]
pub(crate) fn sgarea(x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
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

/// Compute overlap area between a convex quadrilateral and a pixel cell.
///
/// Shifts the quadrilateral so that the cell with lower-left corner `(ox, oy)` becomes the
/// unit square [0,1]×[0,1], then sums signed areas from each edge via `sgarea()`.
///
/// Port of STScI `boxer()` from cdrizzlebox.c. Output pixels are integer-center (pixel `o`
/// spans `[o - 0.5, o + 0.5]`, matching STScI), so callers pass the cell's lower-left
/// corner `o - 0.5`.
#[inline]
pub(crate) fn boxer(ox: f64, oy: f64, x: &[f64; 4], y: &[f64; 4]) -> f64 {
    let px = [x[0] - ox, x[1] - ox, x[2] - ox, x[3] - ox];
    let py = [y[0] - oy, y[1] - oy, y[2] - oy, y[3] - oy];

    let mut sum = 0.0;
    for i in 0..4 {
        let j = (i + 1) & 3;
        sum += sgarea(px[i], py[i], px[j], py[j]);
    }
    sum.abs()
}
