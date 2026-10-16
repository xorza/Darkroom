//! Polynomial distortion correction in the form of SIP (Simple Imaging Polynomial).
//!
//! FITS WCS headers carry SIP's polynomials (Shupe et al. 2005) from pixels to intermediate world
//! coordinates. This uses their form between two frames instead: it corrects reference pixels
//! before the linear transform to a target frame, holds the coefficients in coordinates normalized
//! about the matched stars' centroid, and has no inverse polynomial (`AP`/`BP`). It is not a WCS
//! SIP solution, and no header is written from it.
//!
//! # Model
//!
//! Pixel coordinates (u, v) relative to a reference point are corrected by a 2D
//! polynomial before the linear transform, up to an affine map:
//!
//! ```text
//! u' = u + Σ A_pq * u^p * v^q    (for 2 ≤ p+q ≤ order)
//! v' = v + Σ B_pq * u^p * v^q    (for 2 ≤ p+q ≤ order)
//! ```
//!
//! Linear terms (p+q < 2) are excluded because they are already captured by
//! the linear transform.
//!
//! # Coefficient counts by order
//!
//! | Order | Terms per axis | Description |
//! |-------|---------------|-------------|
//! | 2     | 3             | Barrel/pincushion (u², uv, v²) |
//! | 3     | 7             | + mustache distortion |
//! | 4     | 12            | + higher-order |
//! | 5     | 18            | Full SIP (HST-level) |

pub(crate) mod sip_order;

use glam::{DMat2, DVec2};
use nalgebra::DMatrix;

use arrayvec::ArrayVec;

use crate::config_value::ConfigValue;
use crate::error::InvalidConfigField;
use crate::math::lstsq::Lstsq;
use crate::registration::distortion::sip::sip_order::SipOrder;
use crate::registration::point_normalization::PointNormalization;
use crate::registration::transform::Transform;
use crate::registration::transform::linear_model::LinearModel;

/// The highest order a polynomial holds.
const MAX_ORDER: usize = SipOrder::Five.get();

const MAX_TERMS: usize = SipOrder::Five.term_count();

/// Configuration for SIP polynomial fitting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SipConfig {
    /// Polynomial order. Default: 3.
    pub order: SipOrder,

    /// The polynomial's origin, typically the image centre: coordinates are taken relative to it.
    /// `None` takes the centre of the reference catalog's bounding box, which every frame
    /// registered to that reference shares.
    pub reference_point: Option<DVec2>,
}

impl Default for SipConfig {
    fn default() -> Self {
        Self {
            order: SipOrder::Three,
            reference_point: None,
        }
    }
}

impl SipConfig {
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        if let Some(reference_point) = self.reference_point {
            ConfigValue::new("SIP reference_point x", reference_point.x).finite()?;
            ConfigValue::new("SIP reference_point y", reference_point.y).finite()?;
        }
        Ok(())
    }
}

/// SIP polynomial distortion correction.
///
/// Stores the forward correction polynomials: given pixel coordinates (u, v)
/// relative to the reference point, computes the distortion correction
/// (du, dv) to apply before the linear transform.
///
/// Internally, coordinates are normalized for numerical stability. The
/// coefficients are stored in normalized space.
#[derive(Debug, Clone)]
pub struct SipPolynomial {
    norm: PointNormalization,
    order: SipOrder,
    coeffs_u: ArrayVec<f64, MAX_TERMS>,
    coeffs_v: ArrayVec<f64, MAX_TERMS>,
}

/// A SIP correction's order and the origin its polynomial is taken about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SipModel {
    pub(crate) order: SipOrder,
    pub(crate) origin: DVec2,
}

/// Matched pairs as the SIP fits take them, each with its weight.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SipPairs<'a> {
    pub(crate) reference: &'a [DVec2],
    pub(crate) target: &'a [DVec2],
    pub(crate) weights: &'a [f64],
}

/// A transform and the SIP correction fitted together with it.
#[derive(Debug, Clone)]
pub(crate) struct SipFit {
    pub(crate) transform: Transform,
    pub(crate) sip: SipPolynomial,
}

/// The secant steps a rotation's joint fit may take: it converges superlinearly from the
/// unconstrained fit's angle, to rounding within a handful.
const MAX_SECANT_STEPS: usize = 50;

/// The pairs a SIP fit solves over, and the polynomial's terms and the normalization they are
/// evaluated in.
#[derive(Debug)]
struct SipSystem<'a> {
    pairs: SipPairs<'a>,
    order: SipOrder,
    norm: PointNormalization,
}

/// A joint fit's solution in pixels: the warp `A·r + t₀ + H(u)`, as its linear part `A`, its
/// constant `t₀`, and the coefficients `h` of `H`, one per term.
#[derive(Debug)]
struct JointSolution {
    linear: DMat2,
    constant: DVec2,
    h: Vec<DVec2>,
}

/// A fit with its linear part held, and the derivative of its χ² in a rotation's angle.
#[derive(Debug)]
struct FixedLinearFit {
    fit: SipFit,
    angle_derivative: f64,
}

impl SipPolynomial {
    /// A transform of `model` and its correction `sip`, fitted together to the joint optimum, as
    /// astrometry.net's `fit_sip_wcs` fits a linear part and its correction: a correction `c`
    /// carried through a linear part `A` adds `A·c`, so with `H = A·c` the warp `A·r + t₀ + H(u)`
    /// is linear in everything but a rotation's angle, and `c` is `A⁻¹·H`.
    ///
    /// A translation holds `A = I` and a similarity `A = [a −b; b a]`, both linear; an affine map
    /// frees `A`. A rotation's angle is not linear: for a fixed angle the rest is, and by the
    /// envelope theorem the derivative of the optimum's χ² in the angle is `2·Σ wᵢ·eᵢ·R′·rᵢ` at it,
    /// which a secant iteration drives to zero from the similarity's angle. A homography has no
    /// SIP fit: its perspective terms act to first order as the correction's quadratic ones, and
    /// the two are not determined together — which is why the SIP convention puts an affine map
    /// under the polynomial, and why a [`LinearModel`] stops at one. `None` when the pairs do not
    /// determine the fit, or its linear part is singular.
    pub(crate) fn fit_with(
        model: LinearModel,
        pairs: SipPairs<'_>,
        sip: SipModel,
    ) -> Option<SipFit> {
        let system = SipSystem::new(pairs, sip);
        match model {
            LinearModel::Translation => system
                .fit_fixed(LinearModel::Translation, DMat2::IDENTITY)
                .map(|fixed| fixed.fit),
            LinearModel::Euclidean => system.fit_rotation(),
            LinearModel::Similarity => system.fit_similarity(),
            LinearModel::Affine => system.fit_affine(),
        }
    }

    /// Apply the SIP correction to a point.
    pub fn correct(&self, p: DVec2) -> DVec2 {
        p + self.correction_at(p)
    }

    /// The Jacobian of [`Self::correct`] at `p`: the identity plus the polynomial's derivative.
    pub fn jacobian(&self, p: DVec2) -> DMat2 {
        self.local(p).jacobian
    }

    /// [`Self::correct`] and [`Self::jacobian`] at `p` from one set of powers, as a Newton step
    /// takes both.
    ///
    /// The correction is `s·P((p − p₀)/s)` in the normalized coordinates the polynomial is held in,
    /// so the scale cancels and its derivative is `P`'s: `∂(uᵖvᵠ)/∂u = p·uᵖ⁻¹vᵠ`, from the same
    /// [`MonomialPowers`] [`evaluate_basis`] reads. Columns are the images of the x and y steps.
    pub(crate) fn local(&self, p: DVec2) -> SipLocal {
        let powers = MonomialPowers::at(self.norm.normalize(p));
        let mut value = DVec2::ZERO;
        let mut d_du = DVec2::ZERO;
        let mut d_dv = DVec2::ZERO;
        for (i, &(pu, pv)) in self.order.terms().iter().enumerate() {
            let coefficients = DVec2::new(self.coeffs_u[i], self.coeffs_v[i]);
            value += coefficients * (powers.u[pu] * powers.v[pv]);
            if pu > 0 {
                d_du += coefficients * (pu as f64 * powers.u[pu - 1] * powers.v[pv]);
            }
            if pv > 0 {
                d_dv += coefficients * (pv as f64 * powers.u[pu] * powers.v[pv - 1]);
            }
        }
        SipLocal {
            corrected: p + self.norm.denormalize_delta(value),
            jacobian: DMat2::from_cols(DVec2::X + d_du, DVec2::Y + d_dv),
        }
    }

    /// The correction along pixel row `y`, as two polynomials in `u` alone.
    ///
    /// With `v` fixed, `Σ cₚᵩ·uᵖ·vᵠ` is `Σₚ (Σᵩ cₚᵩ·vᵠ)·uᵖ`: the inner sums are taken once per row,
    /// and each pixel costs one Horner pass of `order` steps per axis instead of every monomial.
    /// Exact as algebra; the reordered sums round differently from [`Self::correct`] in the last
    /// bits.
    pub(crate) fn row(&self, y: f64) -> SipRow {
        let v = self.norm.normalize(DVec2::new(0.0, y)).y;
        let order = self.order.get();
        let mut v_powers = [1.0; MAX_ORDER + 1];
        for k in 1..=MAX_ORDER {
            v_powers[k] = v_powers[k - 1] * v;
        }
        let mut coefficients = [DVec2::ZERO; MAX_ORDER + 1];
        for (i, &(p, q)) in self.order.terms().iter().enumerate() {
            coefficients[p] += DVec2::new(self.coeffs_u[i], self.coeffs_v[i]) * v_powers[q];
        }
        SipRow {
            y,
            center_x: self.norm.center().x,
            scale: self.norm.scale(),
            order,
            coefficients,
        }
    }

    /// Compute the correction vector at a point (without applying it).
    fn correction_at(&self, p: DVec2) -> DVec2 {
        let terms = self.order.terms();
        let mut basis = [0.0; MAX_TERMS];
        evaluate_basis(self.norm.normalize(p), terms, &mut basis[..terms.len()]);

        let mut du = 0.0;
        let mut dv = 0.0;
        for (i, &b) in basis[..terms.len()].iter().enumerate() {
            du += self.coeffs_u[i] * b;
            dv += self.coeffs_v[i] * b;
        }

        self.norm.denormalize_delta(DVec2::new(du, dv))
    }
}

impl<'a> SipSystem<'a> {
    fn new(pairs: SipPairs<'a>, sip: SipModel) -> Self {
        Self {
            pairs,
            order: sip.order,
            norm: PointNormalization::around(pairs.reference, sip.origin),
        }
    }

    /// The solved warp and its correction `A⁻¹·H`, the transform typed as the `model` it was
    /// fitted as.
    fn assemble(&self, model: LinearModel, solution: JointSolution) -> Option<SipFit> {
        let JointSolution {
            linear,
            constant,
            h,
        } = solution;
        let determinant = linear.determinant();
        if determinant == 0.0 || !determinant.is_finite() {
            return None;
        }
        // The correction is held in normalized units, `scale` times smaller than in pixels.
        let inverse = linear.inverse() * (1.0 / self.norm.scale());
        let mut coeffs_u = ArrayVec::new();
        let mut coeffs_v = ArrayVec::new();
        for &term in &h {
            let coefficient = inverse * term;
            coeffs_u.push(coefficient.x);
            coeffs_v.push(coefficient.y);
        }
        let angle = linear.x_axis.y.atan2(linear.x_axis.x);
        let transform = match model {
            LinearModel::Translation => Transform::translation(constant),
            LinearModel::Euclidean => Transform::euclidean(constant, angle),
            LinearModel::Similarity => {
                Transform::similarity(constant, angle, linear.x_axis.length())
            }
            LinearModel::Affine => Transform::affine([
                linear.x_axis.x,
                linear.y_axis.x,
                constant.x,
                linear.x_axis.y,
                linear.y_axis.y,
                constant.y,
            ]),
        };
        transform.is_valid().then(|| SipFit {
            transform,
            sip: SipPolynomial {
                norm: self.norm,
                order: self.order,
                coeffs_u,
                coeffs_v,
            },
        })
    }

    /// With the pixel linear part held at `linear`, the constant and the terms by one weighted
    /// solve per axis on `t − A·r`, and the solved warp's weighted χ² and its residuals' first
    /// moment against `rotate(r)`: the angle derivative a rotation's fit needs.
    fn fit_fixed(&self, model: LinearModel, linear: DMat2) -> Option<FixedLinearFit> {
        let k = self.order.term_count();
        let n = self.pairs.reference.len();
        let mut design = DMatrix::zeros(n, 1 + k);
        let mut rhs = DMatrix::zeros(n, 2);
        let mut basis = [0.0; MAX_TERMS];
        for (row, ((&r, &t), &w)) in self
            .pairs
            .reference
            .iter()
            .zip(self.pairs.target)
            .zip(self.pairs.weights)
            .enumerate()
        {
            let root = w.sqrt();
            evaluate_basis(self.norm.normalize(r), self.order.terms(), &mut basis[..k]);
            design[(row, 0)] = root;
            for (column, &value) in basis[..k].iter().enumerate() {
                design[(row, 1 + column)] = root * value;
            }
            let rest = t - linear * r;
            rhs[(row, 0)] = root * rest.x;
            rhs[(row, 1)] = root * rest.y;
        }
        let solution = Lstsq::new(design).solve(&rhs)?;
        let constant = DVec2::new(solution[(0, 0)], solution[(0, 1)]);
        let h: Vec<DVec2> = (0..k)
            .map(|i| DVec2::new(solution[(1 + i, 0)], solution[(1 + i, 1)]))
            .collect();
        let mut angle_derivative = 0.0;
        for ((&r, &t), &w) in self
            .pairs
            .reference
            .iter()
            .zip(self.pairs.target)
            .zip(self.pairs.weights)
        {
            evaluate_basis(self.norm.normalize(r), self.order.terms(), &mut basis[..k]);
            let correction = h
                .iter()
                .zip(&basis[..k])
                .fold(DVec2::ZERO, |sum, (&term, &value)| sum + term * value);
            let residual = linear * r + constant + correction - t;
            // `R′(θ)·r` is the quarter turn of `R(θ)·r`.
            angle_derivative += 2.0 * w * residual.dot((linear * r).perp());
        }
        Some(FixedLinearFit {
            fit: self.assemble(
                model,
                JointSolution {
                    linear,
                    constant,
                    h,
                },
            )?,
            angle_derivative,
        })
    }

    /// The rotation's angle by the secant method on the derivative of the optimum's χ², from the
    /// similarity's angle, to the rounding of the angle.
    fn fit_rotation(&self) -> Option<SipFit> {
        let start = self
            .fit_similarity()?
            .transform
            .local_geometry(DVec2::ZERO)
            .rotation;
        let at = |angle: f64| {
            let (sin, cos) = angle.sin_cos();
            self.fit_fixed(
                LinearModel::Euclidean,
                DMat2::from_cols_array(&[cos, sin, -sin, cos]),
            )
        };
        let (mut previous_angle, mut previous) = (start, at(start)?);
        let mut angle = start + 1e-6;
        let mut current = at(angle)?;
        for _ in 0..MAX_SECANT_STEPS {
            let slope = current.angle_derivative - previous.angle_derivative;
            if current.angle_derivative == 0.0 || slope == 0.0 {
                break;
            }
            let next = angle - current.angle_derivative * (angle - previous_angle) / slope;
            let step = (next - angle).abs();
            previous_angle = angle;
            previous = current;
            angle = next;
            current = at(angle)?;
            if step <= 4.0 * f64::EPSILON * angle.abs().max(1.0) {
                break;
            }
        }
        Some(current.fit)
    }

    /// `A = [a −b; b a]`: the two shared coefficients couple the axes, so both rows of every pair
    /// enter one solve.
    fn fit_similarity(&self) -> Option<SipFit> {
        let k = self.order.term_count();
        let n = self.pairs.reference.len();
        let mut design = DMatrix::zeros(2 * n, 4 + 2 * k);
        let mut rhs = DMatrix::zeros(2 * n, 1);
        let mut basis = [0.0; MAX_TERMS];
        for (i, ((&r, &t), &w)) in self
            .pairs
            .reference
            .iter()
            .zip(self.pairs.target)
            .zip(self.pairs.weights)
            .enumerate()
        {
            let root = w.sqrt();
            let u = self.norm.normalize(r);
            evaluate_basis(u, self.order.terms(), &mut basis[..k]);
            let (x, y) = (2 * i, 2 * i + 1);
            design[(x, 0)] = root;
            design[(y, 1)] = root;
            design[(x, 2)] = root * u.x;
            design[(x, 3)] = -root * u.y;
            design[(y, 2)] = root * u.y;
            design[(y, 3)] = root * u.x;
            for (column, &value) in basis[..k].iter().enumerate() {
                design[(x, 4 + column)] = root * value;
                design[(y, 4 + k + column)] = root * value;
            }
            rhs[(x, 0)] = root * t.x;
            rhs[(y, 0)] = root * t.y;
        }
        let solution = Lstsq::new(design).solve(&rhs)?;
        let (a, b) = (solution[(2, 0)], solution[(3, 0)]);
        let linear =
            DMat2::from_cols(DVec2::new(a, b), DVec2::new(-b, a)) * (1.0 / self.norm.scale());
        let constant = DVec2::new(solution[(0, 0)], solution[(1, 0)]) - linear * self.norm.center();
        let h: Vec<DVec2> = (0..k)
            .map(|i| DVec2::new(solution[(4 + i, 0)], solution[(4 + k + i, 0)]))
            .collect();
        self.assemble(
            LinearModel::Similarity,
            JointSolution {
                linear,
                constant,
                h,
            },
        )
    }

    /// A free `A`: one weighted solve per axis of the full polynomial, constant and linear terms
    /// included.
    fn fit_affine(&self) -> Option<SipFit> {
        let k = self.order.term_count();
        let n = self.pairs.reference.len();
        let mut design = DMatrix::zeros(n, 3 + k);
        let mut rhs = DMatrix::zeros(n, 2);
        let mut basis = [0.0; MAX_TERMS];
        for (row, ((&r, &t), &w)) in self
            .pairs
            .reference
            .iter()
            .zip(self.pairs.target)
            .zip(self.pairs.weights)
            .enumerate()
        {
            let root = w.sqrt();
            let u = self.norm.normalize(r);
            evaluate_basis(u, self.order.terms(), &mut basis[..k]);
            design[(row, 0)] = root;
            design[(row, 1)] = root * u.x;
            design[(row, 2)] = root * u.y;
            for (column, &value) in basis[..k].iter().enumerate() {
                design[(row, 3 + column)] = root * value;
            }
            rhs[(row, 0)] = root * t.x;
            rhs[(row, 1)] = root * t.y;
        }
        let solution = Lstsq::new(design).solve(&rhs)?;
        // In pixels the linear part is the normalized one over the scale, about the origin.
        let linear = DMat2::from_cols(
            DVec2::new(solution[(1, 0)], solution[(1, 1)]),
            DVec2::new(solution[(2, 0)], solution[(2, 1)]),
        ) * (1.0 / self.norm.scale());
        let constant = DVec2::new(solution[(0, 0)], solution[(0, 1)]) - linear * self.norm.center();
        let h: Vec<DVec2> = (0..k)
            .map(|i| DVec2::new(solution[(3 + i, 0)], solution[(3 + i, 1)]))
            .collect();
        self.assemble(
            LinearModel::Affine,
            JointSolution {
                linear,
                constant,
                h,
            },
        )
    }
}

/// [`SipPolynomial::local`]: the corrected point and the correction's Jacobian there.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SipLocal {
    pub(crate) corrected: DVec2,
    pub(crate) jacobian: DMat2,
}

/// [`SipPolynomial::row`]: the correction along one pixel row, its coefficients collapsed to `u`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SipRow {
    y: f64,
    center_x: f64,
    scale: f64,
    order: usize,
    /// `Σᵩ cₚᵩ·vᵠ` for each power `p` of `u`, both axes.
    coefficients: [DVec2; MAX_ORDER + 1],
}

impl SipRow {
    /// The corrected point of pixel column `x` on this row.
    #[inline]
    pub(crate) fn correct(&self, x: f64) -> DVec2 {
        let u = (x - self.center_x) / self.scale;
        let correction = self.coefficients[..=self.order]
            .iter()
            .rev()
            .fold(DVec2::ZERO, |sum, &coefficient| sum * u + coefficient);
        DVec2::new(x, self.y) + correction * self.scale
    }
}

/// The powers `u⁰..u^MAX_ORDER` and `v⁰..v^MAX_ORDER` of one normalized point, built by repeated
/// multiplication: two multiplies per order.
#[derive(Debug, Clone, Copy)]
struct MonomialPowers {
    u: [f64; MAX_ORDER + 1],
    v: [f64; MAX_ORDER + 1],
}

impl MonomialPowers {
    #[inline]
    fn at(uv: DVec2) -> Self {
        let mut powers = Self {
            u: [1.0; MAX_ORDER + 1],
            v: [1.0; MAX_ORDER + 1],
        };
        for k in 1..=MAX_ORDER {
            powers.u[k] = powers.u[k - 1] * uv.x;
            powers.v[k] = powers.v[k - 1] * uv.y;
        }
        powers
    }
}

/// Every monomial `u^p·v^q` of `terms` at a normalized point, one multiply per term over its
/// [`MonomialPowers`].
#[inline]
fn evaluate_basis(uv: DVec2, terms: &[(usize, usize)], basis: &mut [f64]) {
    let powers = MonomialPowers::at(uv);
    for (value, &(p, q)) in basis.iter_mut().zip(terms) {
        *value = powers.u[p] * powers.v[q];
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use super::*;
    use crate::math::size2us::Size2us;
    use crate::registration::transform::TransformType;

    /// A fit's u and v coefficients.
    #[derive(Debug)]
    struct Coefficients {
        u: ArrayVec<f64, MAX_TERMS>,
        v: ArrayVec<f64, MAX_TERMS>,
    }

    impl SipModel {
        /// The correction this model fits under `transform` from `reference` onto `target`, every
        /// pair weighed alike: the fixture a warp test starts from.
        pub(crate) fn fitted_under(
            self,
            transform: &Transform,
            reference: &[DVec2],
            target: &[DVec2],
        ) -> SipPolynomial {
            let weights = vec![1.0; reference.len()];
            let pairs = SipPairs {
                reference,
                target,
                weights: &weights,
            };
            SipPolynomial::fit_under(transform, pairs, self).expect("a fixture's pairs fit")
        }
    }

    impl SipSystem<'_> {
        /// The u and v coefficients apart, on the reference-frame targets `J⁻¹·(t − T(r))` at
        /// weights `w·|det J|`.
        fn conformal(&self, transform: &Transform) -> Option<Coefficients> {
            let k = self.order.term_count();
            let n = self.pairs.reference.len();
            let mut design = DMatrix::zeros(n, k);
            let mut rhs = DMatrix::zeros(n, 2);
            let mut basis = [0.0; MAX_TERMS];
            for (row, ((&r, &t), &w)) in self
                .pairs
                .reference
                .iter()
                .zip(self.pairs.target)
                .zip(self.pairs.weights)
                .enumerate()
            {
                let jacobian = transform.jacobian(r);
                let root = (w * jacobian.determinant().abs()).sqrt();
                evaluate_basis(self.norm.normalize(r), self.order.terms(), &mut basis[..k]);
                let target = self
                    .norm
                    .normalize_delta(jacobian.inverse() * (t - transform.apply(r)));
                for (column, &value) in basis[..k].iter().enumerate() {
                    design[(row, column)] = root * value;
                }
                rhs[(row, 0)] = root * target.x;
                rhs[(row, 1)] = root * target.y;
            }
            let solution = Lstsq::new(design).solve(&rhs)?;
            Some(Coefficients {
                u: solution.column(0).iter().copied().collect(),
                v: solution.column(1).iter().copied().collect(),
            })
        }

        /// Both coefficient sets in one system: each pair's two rows are `√w·J` applied to the
        /// correction's basis, against `√w·(t − T(r))`.
        fn coupled(&self, transform: &Transform) -> Option<Coefficients> {
            let k = self.order.term_count();
            let n = self.pairs.reference.len();
            let mut design = DMatrix::zeros(2 * n, 2 * k);
            let mut rhs = DMatrix::zeros(2 * n, 1);
            let mut basis = [0.0; MAX_TERMS];
            for (i, ((&r, &t), &w)) in self
                .pairs
                .reference
                .iter()
                .zip(self.pairs.target)
                .zip(self.pairs.weights)
                .enumerate()
            {
                let root = w.sqrt();
                evaluate_basis(self.norm.normalize(r), self.order.terms(), &mut basis[..k]);
                let jacobian = transform.jacobian(r);
                // A correction held in normalized units is `scale` times larger in pixels, so the
                // residual is divided by the scale instead of every column multiplied by it.
                let residual = self.norm.normalize_delta(t - transform.apply(r));
                for (column, &value) in basis[..k].iter().enumerate() {
                    design[(2 * i, column)] = root * jacobian.x_axis.x * value;
                    design[(2 * i, k + column)] = root * jacobian.y_axis.x * value;
                    design[(2 * i + 1, column)] = root * jacobian.x_axis.y * value;
                    design[(2 * i + 1, k + column)] = root * jacobian.y_axis.y * value;
                }
                rhs[(2 * i, 0)] = root * residual.x;
                rhs[(2 * i + 1, 0)] = root * residual.y;
            }
            let solution = Lstsq::new(design).solve(&rhs)?;
            Some(Coefficients {
                u: (0..k).map(|i| solution[(i, 0)]).collect(),
                v: (0..k).map(|i| solution[(k + i, 0)]).collect(),
            })
        }
    }

    impl SipPolynomial {
        /// The correction `sip` that `transform` warps the pairs best by: the weighted least
        /// squares of `T(r) + J(r)·c(r) − t` in target pixels. Exact for an affine `T`, whose
        /// `T(r + c)` is `T(r) + J·c`; first order for a homography. `None` when the pairs do not
        /// determine it.
        ///
        /// A conformal `T` — translation, rotation, similarity — has `JᵀJ = s²·I`, so the residual
        /// separates exactly into the reference frame, `c − J⁻¹·(t − T(r))` at weight `w·s²`, and
        /// the u and v coefficients solve apart. Any other `T` couples them through `J`, and the
        /// system carries both.
        pub(crate) fn fit_under(
            transform: &Transform,
            pairs: SipPairs<'_>,
            sip: SipModel,
        ) -> Option<Self> {
            let system = SipSystem::new(pairs, sip);
            let conformal = matches!(
                transform.transform_type(),
                TransformType::Translation | TransformType::Euclidean | TransformType::Similarity
            );
            let coefficients = if conformal {
                system.conformal(transform)?
            } else {
                system.coupled(transform)?
            };
            Some(Self {
                norm: system.norm,
                order: system.order,
                coeffs_u: coefficients.u,
                coeffs_v: coefficients.v,
            })
        }

        /// The corrected residual of each pair: `|T(r + c(r)) − t|`, in target pixels.
        pub(crate) fn corrected_residuals(
            &self,
            ref_points: &[DVec2],
            target_points: &[DVec2],
            transform: &Transform,
        ) -> Vec<f64> {
            ref_points
                .iter()
                .zip(target_points)
                .map(|(&r, &t)| (transform.apply(self.correct(r)) - t).length())
                .collect()
        }

        /// The largest correction over a grid of `grid_spacing` across `size`, its far edges
        /// included.
        #[expect(
            clippy::cast_sign_loss,
            reason = "a size over a positive grid spacing is non-negative"
        )]
        pub(crate) fn max_grid_correction(&self, size: Size2us, grid_spacing: f64) -> f64 {
            // Integer-stepped to avoid float accumulation drift skipping the boundary band.
            let nx = (size.width as f64 / grid_spacing).floor() as usize;
            let ny = (size.height as f64 / grid_spacing).floor() as usize;
            let mut max_mag = 0.0f64;
            for iy in 0..=ny {
                for ix in 0..=nx {
                    let point = DVec2::new(ix as f64 * grid_spacing, iy as f64 * grid_spacing);
                    max_mag = max_mag.max(self.correction_at(point).length());
                }
            }
            max_mag
        }
    }
}

#[cfg(test)]
mod tests;
