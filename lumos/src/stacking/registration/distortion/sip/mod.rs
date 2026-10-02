//! SIP (Simple Imaging Polynomial) distortion correction.
//!
//! The SIP convention is the standard in astronomy for representing non-linear
//! geometric distortion in FITS image headers. It is used by Spitzer, HST,
//! Astrometry.net, Siril, and ASTAP.
//!
//! # Model
//!
//! Pixel coordinates (u, v) relative to a reference point are corrected by a 2D
//! polynomial before the linear (CD matrix / homography) transform:
//!
//! ```text
//! u' = u + Σ A_pq * u^p * v^q    (for 2 ≤ p+q ≤ order)
//! v' = v + Σ B_pq * u^p * v^q    (for 2 ≤ p+q ≤ order)
//! ```
//!
//! Linear terms (p+q < 2) are excluded because they are already captured by
//! the homography / CD matrix.
//!
//! # Coefficient counts by order
//!
//! | Order | Terms per axis | Description |
//! |-------|---------------|-------------|
//! | 2     | 3             | Barrel/pincushion (u², uv, v²) |
//! | 3     | 7             | + mustache distortion |
//! | 4     | 12            | + higher-order |
//! | 5     | 18            | Full SIP (HST-level) |

use arrayvec::ArrayVec;
use glam::DVec2;

use nalgebra::DMatrix;

use crate::error::InvalidConfigField;
use crate::math::size2us::Size2us;
use crate::math::statistics::{MAD_TO_SIGMA, mad_fast, median_fast};
use crate::stacking::registration::point_normalization::{PointNormalization, centroid};
use crate::stacking::registration::result::RegistrationError;
use crate::stacking::registration::transform::Transform;

/// The highest polynomial order [`SipConfig::order`] accepts.
const MAX_ORDER: usize = 5;

/// Maximum number of polynomial terms (order 5): (5+1)(5+2)/2 - 3 = 18.
const MAX_TERMS: usize = 18;

/// The corrected-residual scale below which clipping stops: a mapped coordinate up to 2²⁰ px is
/// resolved to `u·2²⁰` ≈ 1.2e-10 px in f64, and a corrected residual takes a handful of operations
/// at that scale, so a spread under 1e-9 px is rounding rather than outliers.
const RESIDUAL_RESOLUTION_PX: f64 = 1e-9;

/// Configuration for SIP polynomial fitting.
#[derive(Debug, Clone)]
pub struct SipConfig {
    /// Polynomial order (2-5). Order 2 handles barrel/pincushion,
    /// order 3 handles mustache distortion.
    pub order: usize,

    /// Reference point for the polynomial (typically image center).
    /// Coordinates are relative to this point before polynomial evaluation.
    /// If None, the centroid of the input points is used.
    pub reference_point: Option<DVec2>,

    /// Sigma threshold for iterative outlier rejection (default 3.0).
    /// Points with residuals beyond `clip_sigma * MAD_sigma` are rejected.
    pub clip_sigma: f64,

    /// Number of sigma-clipping iterations (default 3). Set to 0 to disable.
    pub clip_iterations: usize,
}

impl Default for SipConfig {
    fn default() -> Self {
        Self {
            order: 3,
            reference_point: None,
            clip_sigma: 3.0,
            clip_iterations: 3,
        }
    }
}

impl SipConfig {
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::check(
            (2..=MAX_ORDER).contains(&self.order),
            "SIP order",
            "between 2 and 5",
            self.order as f64,
        )?;
        InvalidConfigField::finite(
            "SIP clip_sigma",
            "finite and positive",
            self.clip_sigma,
            |value| value > 0.0,
        )?;
        if let Some(reference_point) = self.reference_point {
            InvalidConfigField::finite_only("SIP reference_point x", reference_point.x)?;
            InvalidConfigField::finite_only("SIP reference_point y", reference_point.y)?;
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
    terms: ArrayVec<(usize, usize), MAX_TERMS>,
    coeffs_u: ArrayVec<f64, MAX_TERMS>,
    coeffs_v: ArrayVec<f64, MAX_TERMS>,
}

/// Result of a SIP polynomial fit, including quality diagnostics.
#[derive(Debug, Clone)]
pub struct SipFitResult {
    /// The fitted polynomial.
    pub(crate) polynomial: SipPolynomial,
    /// RMS residual in pixels (after SIP correction, across surviving points).
    pub rms_residual: f64,
    /// Maximum residual in pixels (worst surviving point).
    pub max_residual: f64,
    /// Number of points used in the final fit (after sigma-clipping).
    pub points_used: usize,
    /// Number of points rejected by sigma-clipping.
    pub points_rejected: usize,
    /// Maximum correction magnitude in pixels (across fitted points).
    pub max_correction: f64,
}

impl SipPolynomial {
    /// Fit a SIP polynomial to matched point pairs under `transform`.
    ///
    /// The SIP convention corrects reference pixels before the linear transform: a target is
    /// predicted at `T(r + c(r))`. Each fit target is therefore the reference-frame correction that
    /// carries `r` onto its target, `J(r)⁻¹·(t − T(r))` with `J` the Jacobian of `T` at `r` — exact
    /// for every model up to affine, whose `J` is the constant linear part, and a first-order
    /// linearization for a homography. The corrected residuals `|T(r + c(r)) − t|` are then
    /// measured through `T` itself, in target pixels, and sigma clipping and the reported metrics
    /// both use them.
    ///
    /// # Errors
    ///
    /// Returns an error if `config` fails validation, the point counts differ, there are too few
    /// points for a stable fit, `T` is singular at a reference point, or the polynomial system is
    /// singular.
    pub fn fit_from_transform(
        ref_points: &[DVec2],
        target_points: &[DVec2],
        transform: &Transform,
        config: &SipConfig,
    ) -> Result<SipFitResult, RegistrationError> {
        config.validate()?;
        if ref_points.len() != target_points.len() {
            return Err(RegistrationError::SipPointCountMismatch {
                reference: ref_points.len(),
                target: target_points.len(),
            });
        }

        let n = ref_points.len();
        let terms = term_exponents(config.order);
        // Require at least 3x as many points as polynomial terms to prevent overfitting.
        // Astrometry.net practice: order 4 (12 terms) needs ~36 points minimum. Held after every
        // clipping pass too, which keeps the previous fit rather than refit on fewer.
        let required_points = 3 * terms.len();
        if n < required_points {
            return Err(RegistrationError::InsufficientSipPoints {
                found: n,
                required: required_points,
            });
        }

        let ref_pt = config
            .reference_point
            .unwrap_or_else(|| centroid(ref_points));
        let norm = PointNormalization::around(ref_points, ref_pt);

        let mut targets = Vec::with_capacity(n);
        for (&r, &t) in ref_points.iter().zip(target_points) {
            let jacobian = transform.jacobian(r);
            let determinant = jacobian.determinant();
            if determinant == 0.0 || !determinant.is_finite() {
                return Err(RegistrationError::SingularSipSystem);
            }
            targets.push(norm.normalize_delta(jacobian.inverse() * (t - transform.apply(r))));
        }

        let mut mask = vec![true; n];
        let mut polynomial = Self::solve(ref_points, &targets, &mask, norm, &terms)
            .ok_or(RegistrationError::SingularSipSystem)?;
        let mut residuals = Vec::with_capacity(n);
        polynomial.residuals_into(ref_points, target_points, transform, &mut residuals);

        // Iterative sigma clipping on the corrected residuals. The buffers live outside the loop
        // and are refilled each pass, so the iteration allocates nothing but the refit.
        let mut active: Vec<f64> = Vec::with_capacity(n);
        let mut deviations: Vec<f64> = Vec::with_capacity(n);
        let mut candidate_mask = mask.clone();
        for _ in 0..config.clip_iterations {
            active.clear();
            active.extend(
                residuals
                    .iter()
                    .zip(&mask)
                    .filter(|(_, kept)| **kept)
                    .map(|(residual, _)| *residual),
            );
            let median = median_fast(&mut active);
            let mad = mad_fast(&active, median, &mut deviations);
            let threshold = config.clip_sigma * mad * MAD_TO_SIGMA;
            if threshold < RESIDUAL_RESOLUTION_PX {
                break;
            }

            for ((candidate, &kept), &residual) in
                candidate_mask.iter_mut().zip(&mask).zip(&residuals)
            {
                *candidate = kept && residual <= median + threshold;
            }
            let survivors = candidate_mask.iter().filter(|&&kept| kept).count();
            if candidate_mask == mask || survivors < required_points {
                break;
            }
            let Some(refit) = Self::solve(ref_points, &targets, &candidate_mask, norm, &terms)
            else {
                break;
            };
            polynomial = refit;
            mask.copy_from_slice(&candidate_mask);
            polynomial.residuals_into(ref_points, target_points, transform, &mut residuals);
        }

        let points_used = mask.iter().filter(|&&kept| kept).count();
        let mut sum_sq = 0.0;
        let mut max_residual = 0.0f64;
        let mut max_correction = 0.0f64;
        for ((&r, &residual), _) in ref_points
            .iter()
            .zip(&residuals)
            .zip(&mask)
            .filter(|(_, kept)| **kept)
        {
            sum_sq += residual * residual;
            max_residual = max_residual.max(residual);
            max_correction = max_correction.max(polynomial.correction_at(r).length());
        }

        Ok(SipFitResult {
            polynomial,
            rms_residual: (sum_sq / points_used as f64).sqrt(),
            max_residual,
            points_used,
            points_rejected: n - points_used,
            max_correction,
        })
    }

    /// The least-squares polynomial through the masked-in fit targets, or `None` when the design
    /// matrix is rank-deficient.
    ///
    /// Solved on the rectangular design matrix by SVD rather than through its normal equations,
    /// whose condition number is the square of the matrix's. The rank test is the usual numerical
    /// one: a singular value at or below `max(rows, columns)·ε·σ_max` is indistinguishable from
    /// zero in f64.
    fn solve(
        points: &[DVec2],
        targets: &[DVec2],
        mask: &[bool],
        norm: PointNormalization,
        terms: &ArrayVec<(usize, usize), MAX_TERMS>,
    ) -> Option<Self> {
        let n_terms = terms.len();
        let rows = mask.iter().filter(|&&kept| kept).count();
        let mut design = DMatrix::zeros(rows, n_terms);
        let mut rhs = DMatrix::zeros(rows, 2);
        let mut basis = [0.0; MAX_TERMS];
        let kept = points
            .iter()
            .zip(targets)
            .zip(mask)
            .filter(|(_, kept)| **kept);
        for (row, ((&point, &target), _)) in kept.enumerate() {
            evaluate_basis(norm.normalize(point), terms, &mut basis[..n_terms]);
            for (column, &value) in basis[..n_terms].iter().enumerate() {
                design[(row, column)] = value;
            }
            rhs[(row, 0)] = target.x;
            rhs[(row, 1)] = target.y;
        }

        let svd = design.svd(true, true);
        let rank_tolerance = rows.max(n_terms) as f64 * f64::EPSILON * svd.singular_values.max();
        if svd.singular_values.min() <= rank_tolerance {
            return None;
        }
        let solution = svd
            .solve(&rhs, rank_tolerance)
            .expect("an SVD computed with both singular-vector sets can solve");
        Some(Self {
            norm,
            terms: terms.clone(),
            coeffs_u: solution.column(0).iter().copied().collect(),
            coeffs_v: solution.column(1).iter().copied().collect(),
        })
    }

    /// Apply the SIP correction to a point.
    pub fn correct(&self, p: DVec2) -> DVec2 {
        p + self.correction_at(p)
    }

    /// The corrected residual of each pair: `|T(r + c(r)) − t|`, in target pixels.
    pub fn compute_corrected_residuals(
        &self,
        ref_points: &[DVec2],
        target_points: &[DVec2],
        transform: &Transform,
    ) -> Vec<f64> {
        let mut residuals = Vec::with_capacity(ref_points.len());
        self.residuals_into(ref_points, target_points, transform, &mut residuals);
        residuals
    }

    /// [`Self::compute_corrected_residuals`] into a caller's buffer.
    fn residuals_into(
        &self,
        ref_points: &[DVec2],
        target_points: &[DVec2],
        transform: &Transform,
        residuals: &mut Vec<f64>,
    ) {
        residuals.clear();
        residuals.extend(
            ref_points
                .iter()
                .zip(target_points)
                .map(|(&r, &t)| (transform.apply(self.correct(r)) - t).length()),
        );
    }

    /// Get the maximum correction magnitude across a grid of points.
    pub fn max_correction(&self, size: Size2us, grid_spacing: f64) -> f64 {
        assert!(
            grid_spacing > 0.0,
            "grid_spacing must be positive, got {grid_spacing}"
        );
        // Integer-stepped to avoid float accumulation drift skipping the boundary band.
        let nx = (size.width as f64 / grid_spacing).floor() as usize;
        let ny = (size.height as f64 / grid_spacing).floor() as usize;
        let mut max_mag = 0.0f64;
        for iy in 0..=ny {
            let y = iy as f64 * grid_spacing;
            for ix in 0..=nx {
                let x = ix as f64 * grid_spacing;
                let correction = self.correction_at(DVec2::new(x, y));
                max_mag = max_mag.max(correction.length());
            }
        }
        max_mag
    }

    /// Compute the correction vector at a point (without applying it).
    fn correction_at(&self, p: DVec2) -> DVec2 {
        let mut basis = [0.0; MAX_TERMS];
        evaluate_basis(
            self.norm.normalize(p),
            &self.terms,
            &mut basis[..self.terms.len()],
        );

        let mut du = 0.0;
        let mut dv = 0.0;
        for (i, &b) in basis[..self.terms.len()].iter().enumerate() {
            du += self.coeffs_u[i] * b;
            dv += self.coeffs_v[i] * b;
        }

        self.norm.denormalize_delta(DVec2::new(du, dv))
    }
}

/// Generate the list of (p, q) exponent pairs for a given order.
/// Only includes terms where 2 ≤ p+q ≤ order.
fn term_exponents(order: usize) -> ArrayVec<(usize, usize), MAX_TERMS> {
    let mut terms = ArrayVec::new();
    for total in 2..=order {
        for p in (0..=total).rev() {
            let q = total - p;
            terms.push((p, q));
        }
    }
    terms
}

/// Every monomial `u^p·v^q` of `terms` at a normalized point, from one table of powers per axis
/// built by repeated multiplication: two multiplies per order, then one per term.
#[inline]
fn evaluate_basis(uv: DVec2, terms: &[(usize, usize)], basis: &mut [f64]) {
    let mut powers_u = [1.0; MAX_ORDER + 1];
    let mut powers_v = [1.0; MAX_ORDER + 1];
    for k in 1..=MAX_ORDER {
        powers_u[k] = powers_u[k - 1] * uv.x;
        powers_v[k] = powers_v[k - 1] * uv.y;
    }
    for (value, &(p, q)) in basis.iter_mut().zip(terms) {
        *value = powers_u[p] * powers_v[q];
    }
}

#[cfg(test)]
mod tests;
