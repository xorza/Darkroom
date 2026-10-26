//! [`PlaneMoments`]: the least-squares plane through samples at integer offsets, solved exactly.

/// The moments of samples at integer offsets `(x, y)`, for the least-squares plane
/// `v = level + slope_x·x + slope_y·y` through them.
///
/// The offsets are integers, so the position moments and the normal equations' determinant are
/// exact in `i128`, and "on one line" is an exact zero rather than a tolerance. The equations are
/// centred on the samples' mean position, which keeps the values' moments small beside the sums
/// they are taken from.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PlaneMoments {
    n: i128,
    sx: i128,
    sy: i128,
    sxx: i128,
    syy: i128,
    sxy: i128,
    sv: f64,
    sxv: f64,
    syv: f64,
}

/// A plane through samples: its value at the origin of their offsets, and its rise per unit of
/// each.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Plane {
    pub(crate) level: f64,
    pub(crate) slope: [f64; 2],
}

impl Plane {
    /// The plane's value at `offset`.
    pub(crate) fn at(self, offset: [i32; 2]) -> f64 {
        self.level + self.slope[0] * f64::from(offset[0]) + self.slope[1] * f64::from(offset[1])
    }
}

impl PlaneMoments {
    /// Take the sample `value` at `offset`.
    pub(crate) fn add(&mut self, offset: [i32; 2], value: f64) {
        let (x, y) = (i128::from(offset[0]), i128::from(offset[1]));
        self.n += 1;
        self.sx += x;
        self.sy += y;
        self.sxx += x * x;
        self.syy += y * y;
        self.sxy += x * y;
        self.sv += value;
        self.sxv += f64::from(offset[0]) * value;
        self.syv += f64::from(offset[1]) * value;
    }

    /// The least-squares plane through the samples taken; `None` when they span none: fewer than
    /// three, or all on one line.
    pub(crate) fn plane(self) -> Option<Plane> {
        let Self {
            n,
            sx,
            sy,
            sxx,
            syy,
            sxy,
            sv,
            sxv,
            syv,
        } = self;
        // The centred normal equations, each scaled by n: `[cxx cxy; cxy cyy]·slope = [cxv; cyv]`.
        let cxx = n * sxx - sx * sx;
        let cyy = n * syy - sy * sy;
        let cxy = n * sxy - sx * sy;
        let det = cxx * cyy - cxy * cxy;
        if n < 3 || det == 0 {
            return None;
        }
        let (n, sx, sy) = (n as f64, sx as f64, sy as f64);
        let cxv = n * sxv - sx * sv;
        let cyv = n * syv - sy * sv;
        let (cxx, cyy, cxy, det) = (cxx as f64, cyy as f64, cxy as f64, det as f64);
        let slope = [(cxv * cyy - cyv * cxy) / det, (cyv * cxx - cxv * cxy) / det];
        Some(Plane {
            level: (sv - slope[0] * sx - slope[1] * sy) / n,
            slope,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::math::plane_moments::{Plane, PlaneMoments};

    /// Samples on `v = 2 + 0.5x − 0.25y`, every value a dyadic rational, give that plane exactly;
    /// three on one line, or two, give none.
    #[test]
    fn the_plane_through_its_samples_is_exact_and_needs_two_directions() {
        let plane = |offsets: &[[i32; 2]]| {
            let mut moments = PlaneMoments::default();
            for &[x, y] in offsets {
                moments.add([x, y], 2.0 + 0.5 * f64::from(x) - 0.25 * f64::from(y));
            }
            moments.plane()
        };
        assert_eq!(
            plane(&[[-3, -1], [4, 2], [0, 5], [7, -6]]),
            Some(Plane {
                level: 2.0,
                slope: [0.5, -0.25],
            })
        );
        assert_eq!(plane(&[[0, 0], [1, 1], [5, 5]]), None);
        assert_eq!(plane(&[[0, 0], [1, 0]]), None);
    }
}
