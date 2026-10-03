//! The geometric primitives drizzle distributes flux with.
//!
//! The exact polygon-to-pixel overlap the square kernel needs ([`sgarea`] / [`boxer`], ported from
//! `STScI`'s `cdrizzlebox.c`). The interpolating kernel itself is `math::lanczos`, shared with
//! `registration::resample`, and the area a drop is magnified by comes from the transform's own
//! Jacobian.

use glam::DVec2;

const SGAREA_DX_MIN: f64 = 1e-14;

/// Compute signed area between the segment `from → to` and the x-axis, clipped to the unit square
/// `[0,1]×[0,1]`. Uses Green's theorem. Port of `STScI` `sgarea()` from cdrizzlebox.c.
///
/// The sign depends on the direction of traversal (left-to-right = positive).
/// When summed over all 4 edges of a convex quadrilateral (counterclockwise winding),
/// the total gives the overlap area between the quadrilateral and the unit square.
///
/// The body works in components rather than vectors, and deliberately: Green's theorem integrates
/// along x with y as the integrand, so the axes have different jobs — `xlo`/`xhi` clip the segment
/// and `ylo`/`yhi` evaluate it at those clips, and the trapezoid area multiplies an x-difference by
/// a y-sum. Packing those into points would hide which axis each term comes from. Only the two
/// genuinely vectorial steps are vectors: the endpoint difference, and `det` as the cross product
/// of the endpoints.
#[inline]
pub(crate) fn sgarea(from: DVec2, to: DVec2) -> f64 {
    let delta = to - from;
    let (dx, dy) = (delta.x, delta.y);

    if dx.abs() < SGAREA_DX_MIN {
        return 0.0;
    }

    let (sgn_dx, xlo, xhi) = if dx < 0.0 {
        (-1.0, to.x, from.x)
    } else {
        (1.0, from.x, to.x)
    };

    if xlo >= 1.0 || xhi <= 0.0 {
        return 0.0;
    }

    let xlo = xlo.max(0.0);
    let xhi = xhi.min(1.0);

    let slope = dy / dx;
    let ylo = from.y + slope * (xlo - from.x);
    let yhi = from.y + slope * (xhi - from.x);

    if ylo <= 0.0 && yhi <= 0.0 {
        return 0.0;
    }

    if ylo >= 1.0 && yhi >= 1.0 {
        return sgn_dx * (xhi - xlo);
    }

    let det = from.perp_dot(to);

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
/// `[0,1]×[0,1]`, then sums signed areas from each edge via `sgarea()`.
///
/// Port of `STScI` `boxer()` from cdrizzlebox.c. Output pixels are integer-center (pixel `o`
/// spans `[o - 0.5, o + 0.5]`, matching `STScI`), so callers pass the cell's lower-left
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
