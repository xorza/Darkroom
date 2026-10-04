//! [`HomographyRefinement`]: a homography refined on its reprojection error.

use glam::DVec2;

use crate::math::lm_controller::{LmController, LmProblem, NormalEquations};

/// Weighted correspondences in normalized coordinates, as the least-squares problem
/// `Σ wᵢ·|tᵢ − H(rᵢ)|²` over the eight free entries of `H`, its last held at 1.
///
/// The algebraic (DLT) solution minimizes a residual with no geometric meaning, which weights the
/// pairs by how far they sit from the projective line at infinity; Hartley and Zisserman refine it
/// on the reprojection error, which is the noise the positions carry.
#[derive(Debug)]
pub(super) struct HomographyRefinement<'a> {
    pub(super) reference: &'a [DVec2],
    pub(super) target: &'a [DVec2],
    pub(super) weights: &'a [f64],
}

/// One correspondence through `h`: the projected point and the denominator `w`.
#[derive(Debug, Clone, Copy)]
struct Projection {
    point: DVec2,
    w: f64,
}

impl HomographyRefinement<'_> {
    /// The homography from `start`, its entries `[h₀ … h₇]` with `h₈ = 1`; `None` when the
    /// refinement does not converge.
    pub(super) fn refine(&self, start: [f64; 8]) -> Option<[f64; 8]> {
        LmController::STANDARD
            .fit(self, start)
            .map(|fit| fit.params)
    }

    fn project(h: &[f64; 8], r: DVec2) -> Projection {
        let w = h[6] * r.x + h[7] * r.y + 1.0;
        Projection {
            point: DVec2::new(
                (h[0] * r.x + h[1] * r.y + h[2]) / w,
                (h[3] * r.x + h[4] * r.y + h[5]) / w,
            ),
            w,
        }
    }
}

impl LmProblem<8> for HomographyRefinement<'_> {
    /// `∂x′/∂(h₀, h₁, h₂) = (x, y, 1)/w`, `∂y′/∂(h₃, h₄, h₅) = (x, y, 1)/w`, and
    /// `∂(x′, y′)/∂(h₆, h₇) = −(x′, y′)·(x, y)/w`.
    fn normal_equations(&self, params: &[f64; 8]) -> NormalEquations<8> {
        let mut equations = NormalEquations {
            hessian: [[0.0; 8]; 8],
            gradient: [0.0; 8],
            chi2: 0.0,
        };
        for ((&r, &t), &weight) in self.reference.iter().zip(self.target).zip(self.weights) {
            let Projection { point, w } = Self::project(params, r);
            let residual = t - point;
            let rows = [
                [
                    r.x / w,
                    r.y / w,
                    1.0 / w,
                    0.0,
                    0.0,
                    0.0,
                    -r.x * point.x / w,
                    -r.y * point.x / w,
                ],
                [
                    0.0,
                    0.0,
                    0.0,
                    r.x / w,
                    r.y / w,
                    1.0 / w,
                    -r.x * point.y / w,
                    -r.y * point.y / w,
                ],
            ];
            for (row, component) in rows.iter().zip([residual.x, residual.y]) {
                equations.chi2 += weight * component * component;
                for i in 0..8 {
                    equations.gradient[i] += weight * row[i] * component;
                    for j in i..8 {
                        equations.hessian[i][j] += weight * row[i] * row[j];
                    }
                }
            }
        }
        equations.mirror_lower_triangle();
        equations
    }

    fn chi2(&self, params: &[f64; 8]) -> f64 {
        self.reference
            .iter()
            .zip(self.target)
            .zip(self.weights)
            .map(|((&r, &t), &weight)| {
                weight * (t - Self::project(params, r).point).length_squared()
            })
            .sum()
    }

    fn constrain(&self, _params: &mut [f64; 8]) {}
}
