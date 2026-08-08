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

use crate::error::InvalidConfigField;
use crate::math::size2us::Size2us;
use crate::stacking::registration::distortion::SINGULAR_THRESHOLD;
use crate::stacking::registration::distortion::point_normalization::PointNormalization;
use crate::stacking::registration::result::RegistrationError;
use crate::stacking::registration::transform::Transform;

#[cfg(test)]
mod tests;

/// Maximum number of polynomial terms (order 5): (5+1)(5+2)/2 - 3 = 18.
const MAX_TERMS: usize = 18;

/// Maximum size of the A^T*A matrix (flattened).
const MAX_ATA: usize = MAX_TERMS * MAX_TERMS;

/// Maximum size of the LU augmented matrix: MAX_TERMS * (MAX_TERMS + 1).
const MAX_AUG: usize = MAX_TERMS * (MAX_TERMS + 1);

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
            (2..=5).contains(&self.order),
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
    /// Fit SIP polynomial directly from matched point pairs and a transform.
    ///
    /// Given matched inlier positions and a homography, fits a polynomial
    /// correction to minimize residual errors.
    ///
    /// # Errors
    ///
    /// Returns an error if `config` fails validation, the point counts differ,
    /// there are too few points for a stable fit, or the polynomial system is
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
        // Astrometry.net practice: order 4 (12 terms) needs ~36 points minimum.
        let required_points = 3 * terms.len();
        if n < required_points {
            return Err(RegistrationError::InsufficientSipPoints {
                found: n,
                required: required_points,
            });
        }

        let ref_pt = config.reference_point.unwrap_or_else(|| {
            let sum: DVec2 = ref_points.iter().sum();
            sum / n as f64
        });
        let norm = PointNormalization::new(ref_pt, avg_distance(ref_points, ref_pt));

        // Compute target residuals in normalized space (constant across iterations)
        let targets: Vec<DVec2> = ref_points
            .iter()
            .zip(target_points.iter())
            .map(|(&r, &t)| norm.normalize_delta(t - transform.apply(r)))
            .collect();
        let targets_u: Vec<f64> = targets.iter().map(|d| d.x).collect();
        let targets_v: Vec<f64> = targets.iter().map(|d| d.y).collect();

        // Initial fit on all points
        let mut mask = vec![true; n];
        let Some(SipCoefficients {
            u: mut coeffs_u,
            v: mut coeffs_v,
        }) = solve_masked(ref_points, &targets_u, &targets_v, &mask, norm, &terms)
        else {
            return Err(RegistrationError::SingularSipSystem);
        };

        // Iterative sigma-clipping
        for _ in 0..config.clip_iterations {
            // Compute per-point residual magnitudes in normalized space
            let mut residuals: Vec<f64> = Vec::with_capacity(n);
            for i in 0..n {
                if !mask[i] {
                    residuals.push(f64::INFINITY);
                    continue;
                }
                let uv = norm.normalize(ref_points[i]);

                let mut basis = [0.0; MAX_TERMS];
                evaluate_basis(uv, &terms, &mut basis[..terms.len()]);

                let mut pred_u = 0.0;
                let mut pred_v = 0.0;
                for j in 0..terms.len() {
                    pred_u += coeffs_u[j] * basis[j];
                    pred_v += coeffs_v[j] * basis[j];
                }

                let du = pred_u - targets_u[i];
                let dv = pred_v - targets_v[i];
                residuals.push((du * du + dv * dv).sqrt());
            }

            // Compute median and MAD of active residuals
            let mut active: Vec<f64> = residuals
                .iter()
                .zip(mask.iter())
                .filter(|(_, m)| **m)
                .map(|(r, _)| *r)
                .collect();

            if active.len() < terms.len() {
                break; // Not enough points to re-fit
            }

            active.sort_unstable_by(|a, b| a.total_cmp(b));
            let median = active[active.len() / 2];

            let mut deviations: Vec<f64> = active.iter().map(|&r| (r - median).abs()).collect();
            deviations.sort_unstable_by(|a, b| a.total_cmp(b));
            let mad = deviations[deviations.len() / 2];

            const MAD_TO_SIGMA: f64 = 1.4826022;
            let threshold = config.clip_sigma * mad * MAD_TO_SIGMA;

            if threshold < 1e-15 {
                break; // Residuals are essentially zero
            }

            // Reject outliers
            let mut any_rejected = false;
            for i in 0..n {
                if mask[i] && residuals[i] > median + threshold {
                    mask[i] = false;
                    any_rejected = true;
                }
            }

            if !any_rejected {
                break; // Converged
            }

            // Re-fit on surviving points
            let Some(refit) = solve_masked(ref_points, &targets_u, &targets_v, &mask, norm, &terms)
            else {
                return Err(RegistrationError::SingularSipSystem);
            };
            coeffs_u = refit.u;
            coeffs_v = refit.v;
        }

        let polynomial = Self {
            norm,
            terms,
            coeffs_u,
            coeffs_v,
        };

        // Compute quality metrics from final fit
        let points_used = mask.iter().filter(|&&m| m).count();
        let points_rejected = n - points_used;

        let mut sum_sq = 0.0;
        let mut max_residual = 0.0f64;
        let mut max_correction = 0.0f64;
        for i in 0..n {
            if !mask[i] {
                continue;
            }
            let corrected = polynomial.correct(ref_points[i]);
            let mapped = transform.apply(corrected);
            let residual = (mapped - target_points[i]).length();
            sum_sq += residual * residual;
            max_residual = max_residual.max(residual);

            let correction = polynomial.correction_at(ref_points[i]).length();
            max_correction = max_correction.max(correction);
        }
        let rms_residual = if points_used > 0 {
            (sum_sq / points_used as f64).sqrt()
        } else {
            0.0
        };

        Ok(SipFitResult {
            polynomial,
            rms_residual,
            max_residual,
            points_used,
            points_rejected,
            max_correction,
        })
    }

    /// Apply the SIP correction to a point.
    pub fn correct(&self, p: DVec2) -> DVec2 {
        p + self.correction_at(p)
    }

    /// Compute residuals after applying SIP correction.
    ///
    /// For each point, computes `|transform(sip_correct(ref)) - target|`.
    pub fn compute_corrected_residuals(
        &self,
        ref_points: &[DVec2],
        target_points: &[DVec2],
        transform: &Transform,
    ) -> Vec<f64> {
        ref_points
            .iter()
            .zip(target_points.iter())
            .map(|(&r, &t)| {
                let corrected = self.correct(r);
                let mapped = transform.apply(corrected);
                (mapped - t).length()
            })
            .collect()
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
        let uv = self.norm.normalize(p);

        let mut basis = [0.0; MAX_TERMS];
        evaluate_basis(uv, &self.terms, &mut basis[..self.terms.len()]);

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

/// Evaluate a monomial u^p * v^q on a normalized point.
#[inline]
fn monomial(uv: DVec2, p: usize, q: usize) -> f64 {
    uv.x.powi(p as i32) * uv.y.powi(q as i32)
}

/// Evaluate all monomial basis functions for a normalized point.
#[inline]
fn evaluate_basis(uv: DVec2, terms: &[(usize, usize)], basis: &mut [f64]) {
    for (j, &(p, q)) in terms.iter().enumerate() {
        basis[j] = monomial(uv, p, q);
    }
}

/// Compute average distance from a set of points to a reference point.
fn avg_distance(points: &[DVec2], ref_pt: DVec2) -> f64 {
    let sum: f64 = points.iter().map(|p| (*p - ref_pt).length()).sum();
    let avg = sum / points.len() as f64;
    if avg > 1e-10 { avg } else { 1.0 }
}

/// One SIP fit: the polynomial coefficients for each output axis.
///
/// The two axes share a design matrix and differ only in their target values, so they are
/// always solved and consumed together.
#[derive(Debug)]
struct SipCoefficients {
    u: ArrayVec<f64, MAX_TERMS>,
    v: ArrayVec<f64, MAX_TERMS>,
}

/// The normal equations for one SIP fit: `A^T·A` shared by both axes, and one `A^T·b` per axis.
///
/// `ata` is `n_terms × n_terms` row-major in a fixed-capacity buffer, so it carries no dimension
/// of its own — the caller's `terms.len()` is the order.
#[derive(Debug)]
struct SipNormalEquations {
    ata: [f64; MAX_ATA],
    atb_u: [f64; MAX_TERMS],
    atb_v: [f64; MAX_TERMS],
}

/// Solve the SIP normal equations using only the masked-in points.
fn solve_masked(
    points: &[DVec2],
    targets_u: &[f64],
    targets_v: &[f64],
    mask: &[bool],
    norm: PointNormalization,
    terms: &[(usize, usize)],
) -> Option<SipCoefficients> {
    let equations = build_normal_equations(points, targets_u, targets_v, mask, norm, terms);
    let n_terms = terms.len();
    Some(SipCoefficients {
        u: solve_cholesky(&equations.ata, &equations.atb_u, n_terms)?,
        v: solve_cholesky(&equations.ata, &equations.atb_v, n_terms)?,
    })
}

/// Build normal equations A^T*A and A^T*b from point/target pairs (masked).
fn build_normal_equations(
    points: &[DVec2],
    targets_u: &[f64],
    targets_v: &[f64],
    mask: &[bool],
    norm: PointNormalization,
    terms: &[(usize, usize)],
) -> SipNormalEquations {
    let n_terms = terms.len();
    let mut ata = [0.0; MAX_ATA];
    let mut atb_u = [0.0; MAX_TERMS];
    let mut atb_v = [0.0; MAX_TERMS];
    let mut basis = [0.0; MAX_TERMS];

    for (i, point) in points.iter().enumerate() {
        if !mask[i] {
            continue;
        }
        evaluate_basis(norm.normalize(*point), terms, &mut basis[..n_terms]);

        // Accumulate A^T*A and A^T*b
        for j in 0..n_terms {
            for k in j..n_terms {
                let val = basis[j] * basis[k];
                ata[j * n_terms + k] += val;
                if k != j {
                    ata[k * n_terms + j] += val;
                }
            }
            atb_u[j] += basis[j] * targets_u[i];
            atb_v[j] += basis[j] * targets_v[i];
        }
    }

    SipNormalEquations { ata, atb_u, atb_v }
}

/// Solve a symmetric positive definite system Ax = b using Cholesky decomposition.
/// Falls back to LU decomposition if the matrix is not positive definite.
#[allow(clippy::needless_range_loop)]
fn solve_cholesky(a: &[f64], b: &[f64], n: usize) -> Option<ArrayVec<f64, MAX_TERMS>> {
    let mut l = [0.0; MAX_ATA];

    // Cholesky factorization: A = L * L^T
    for i in 0..n {
        for j in 0..=i {
            let mut sum = 0.0;
            for k in 0..j {
                sum += l[i * n + k] * l[j * n + k];
            }

            if i == j {
                let diag = a[i * n + i] - sum;
                if diag <= 0.0 {
                    // Not positive definite, fall back to LU
                    return solve_lu(a, b, n);
                }
                l[i * n + j] = diag.sqrt();
            } else {
                l[i * n + j] = (a[i * n + j] - sum) / l[j * n + j];
            }
        }
    }

    // Condition number estimate: cond(A) ≈ (max(diag(L)) / min(diag(L)))^2.
    // If this exceeds ~1e10 the solution is unreliable; fall back to LU with pivoting.
    let mut diag_min = f64::MAX;
    let mut diag_max = 0.0f64;
    for i in 0..n {
        let d = l[i * n + i];
        diag_min = diag_min.min(d);
        diag_max = diag_max.max(d);
    }
    if diag_min < SINGULAR_THRESHOLD || (diag_max / diag_min) > 1e5 {
        // cond(A) ≈ (1e5)^2 = 1e10, unreliable — fall back to LU
        return solve_lu(a, b, n);
    }

    // Forward substitution: L * y = b
    let mut y = [0.0; MAX_TERMS];
    for i in 0..n {
        let mut sum = 0.0;
        for j in 0..i {
            sum += l[i * n + j] * y[j];
        }
        y[i] = (b[i] - sum) / l[i * n + i];
    }

    // Back substitution: L^T * x = y
    let mut x = [0.0; MAX_TERMS];
    for i in (0..n).rev() {
        let mut sum = 0.0;
        for j in (i + 1)..n {
            sum += l[j * n + i] * x[j];
        }
        x[i] = (y[i] - sum) / l[i * n + i];
    }

    Some(ArrayVec::try_from(&x[..n]).unwrap())
}

/// LU decomposition solver with partial pivoting (fallback for non-positive-definite matrices).
#[allow(clippy::needless_range_loop)]
fn solve_lu(a: &[f64], b: &[f64], n: usize) -> Option<ArrayVec<f64, MAX_TERMS>> {
    // Build augmented matrix [A | b]
    let mut aug = [0.0; MAX_AUG];
    for i in 0..n {
        for j in 0..n {
            aug[i * (n + 1) + j] = a[i * n + j];
        }
        aug[i * (n + 1) + n] = b[i];
    }

    // Gaussian elimination with partial pivoting
    for col in 0..n {
        // Find pivot
        let mut max_row = col;
        let mut max_val = aug[col * (n + 1) + col].abs();
        for row in (col + 1)..n {
            let val = aug[row * (n + 1) + col].abs();
            if val > max_val {
                max_val = val;
                max_row = row;
            }
        }

        if max_val < SINGULAR_THRESHOLD {
            return None; // Singular matrix
        }

        // Swap rows if needed
        if max_row != col {
            for j in 0..=n {
                let idx_a = col * (n + 1) + j;
                let idx_b = max_row * (n + 1) + j;
                aug.swap(idx_a, idx_b);
            }
        }

        // Eliminate below
        for row in (col + 1)..n {
            let factor = aug[row * (n + 1) + col] / aug[col * (n + 1) + col];
            for j in col..=n {
                aug[row * (n + 1) + j] -= factor * aug[col * (n + 1) + j];
            }
        }
    }

    // Back substitution
    let mut x = [0.0; MAX_TERMS];
    for i in (0..n).rev() {
        x[i] = aug[i * (n + 1) + n];
        for j in (i + 1)..n {
            x[i] -= aug[i * (n + 1) + j] * x[j];
        }
        x[i] /= aug[i * (n + 1) + i];
    }

    Some(ArrayVec::try_from(&x[..n]).unwrap())
}
