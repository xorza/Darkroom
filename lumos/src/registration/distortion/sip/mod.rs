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

use crate::registration::transform::Transform;

#[cfg(test)]
mod tests;

// ============================================================================
// Constants
// ============================================================================

/// Maximum number of polynomial terms (order 5): (5+1)(5+2)/2 - 3 = 18.
const MAX_TERMS: usize = 18;

/// Maximum size of the A^T*A matrix (flattened).
const MAX_ATA: usize = MAX_TERMS * MAX_TERMS;

/// Maximum size of the LU augmented matrix: MAX_TERMS * (MAX_TERMS + 1).
const MAX_AUG: usize = MAX_TERMS * (MAX_TERMS + 1);

// ============================================================================
// SipConfig
// ============================================================================

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
    fn validate(&self) {
        assert!(
            (2..=5).contains(&self.order),
            "SIP order must be 2-5, got {}",
            self.order
        );
        assert!(
            self.clip_sigma > 0.0,
            "clip_sigma must be positive, got {}",
            self.clip_sigma
        );
    }
}

// ============================================================================
// SipPolynomial
// ============================================================================

/// SIP polynomial distortion correction.
///
/// Stores the forward correction polynomials: given pixel coordinates (u, v)
/// relative to the reference point, computes the distortion correction
/// (du, dv) to apply before the linear transform.
///
/// Internally, coordinates are normalized by a scale factor for numerical
/// stability. The coefficients are stored in normalized space.
#[derive(Debug, Clone)]
pub struct SipPolynomial {
    reference_point: DVec2,
    norm_scale: f64,
    terms: ArrayVec<(usize, usize), MAX_TERMS>,
    coeffs_u: ArrayVec<f64, MAX_TERMS>,
    coeffs_v: ArrayVec<f64, MAX_TERMS>,
}

/// Result of a SIP polynomial fit, including quality diagnostics.
#[derive(Debug, Clone)]
pub struct SipFitResult {
    /// The fitted polynomial.
    pub polynomial: SipPolynomial,
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
    /// Returns `None` if `ref_points` and `target_points` have different lengths,
    /// or if the system is underdetermined or singular.
    ///
    /// # Panics
    ///
    /// - If `config` fails validation (see [`SipConfig`]: order must be 2..=5).
    pub fn fit_from_transform(
        ref_points: &[DVec2],
        target_points: &[DVec2],
        transform: &Transform,
        config: &SipConfig,
    ) -> Option<SipFitResult> {
        config.validate();
        if ref_points.len() != target_points.len() {
            return None;
        }

        let n = ref_points.len();
        let terms = term_exponents(config.order);
        // Require at least 3x as many points as polynomial terms to prevent overfitting.
        // Astrometry.net practice: order 4 (12 terms) needs ~36 points minimum.
        if n < 3 * terms.len() {
            return None;
        }

        let ref_pt = config.reference_point.unwrap_or_else(|| {
            let sum: DVec2 = ref_points.iter().sum();
            sum / n as f64
        });
        let norm_scale = avg_distance(ref_points, ref_pt);

        // Compute target residuals in normalized space (constant across iterations)
        let targets_u: Vec<f64> = ref_points
            .iter()
            .zip(target_points.iter())
            .map(|(&r, &t)| -(transform.apply(r).x - t.x) / norm_scale)
            .collect();
        let targets_v: Vec<f64> = ref_points
            .iter()
            .zip(target_points.iter())
            .map(|(&r, &t)| -(transform.apply(r).y - t.y) / norm_scale)
            .collect();

        // Initial fit on all points
        let mut mask = vec![true; n];
        let (mut coeffs_u, mut coeffs_v) = solve_masked(
            ref_points, &targets_u, &targets_v, &mask, ref_pt, norm_scale, &terms,
        )?;

        // Iterative sigma-clipping
        for _ in 0..config.clip_iterations {
            // Compute per-point residual magnitudes in normalized space
            let mut residuals: Vec<f64> = Vec::with_capacity(n);
            for i in 0..n {
                if !mask[i] {
                    residuals.push(f64::INFINITY);
                    continue;
                }
                let (u, v) = normalize_point(ref_points[i], ref_pt, norm_scale);

                let mut basis = [0.0; MAX_TERMS];
                evaluate_basis(u, v, &terms, &mut basis[..terms.len()]);

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

            active.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
            let median = active[active.len() / 2];

            let mut deviations: Vec<f64> = active.iter().map(|&r| (r - median).abs()).collect();
            deviations.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
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
            let (new_u, new_v) = solve_masked(
                ref_points, &targets_u, &targets_v, &mask, ref_pt, norm_scale, &terms,
            )?;
            coeffs_u = new_u;
            coeffs_v = new_v;
        }

        let polynomial = Self {
            reference_point: ref_pt,
            norm_scale,
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

        Some(SipFitResult {
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
    pub fn max_correction(&self, width: usize, height: usize, grid_spacing: f64) -> f64 {
        let mut max_mag = 0.0f64;
        let mut y = 0.0;
        while y <= height as f64 {
            let mut x = 0.0;
            while x <= width as f64 {
                let correction = self.correction_at(DVec2::new(x, y));
                max_mag = max_mag.max(correction.length());
                x += grid_spacing;
            }
            y += grid_spacing;
        }
        max_mag
    }

    /// Compute the correction vector at a point (without applying it).
    fn correction_at(&self, p: DVec2) -> DVec2 {
        let (u, v) = normalize_point(p, self.reference_point, self.norm_scale);

        let mut basis = [0.0; MAX_TERMS];
        evaluate_basis(u, v, &self.terms, &mut basis[..self.terms.len()]);

        let mut du = 0.0;
        let mut dv = 0.0;
        for (i, &b) in basis[..self.terms.len()].iter().enumerate() {
            du += self.coeffs_u[i] * b;
            dv += self.coeffs_v[i] * b;
        }

        DVec2::new(du * self.norm_scale, dv * self.norm_scale)
    }
}

// ============================================================================
// Polynomial helpers
// ============================================================================

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

/// Evaluate a monomial u^p * v^q.
#[inline]
fn monomial(u: f64, v: f64, p: usize, q: usize) -> f64 {
    u.powi(p as i32) * v.powi(q as i32)
}

/// Normalize a point relative to the SIP reference point and scale.
#[inline]
fn normalize_point(p: DVec2, ref_pt: DVec2, norm_scale: f64) -> (f64, f64) {
    ((p.x - ref_pt.x) / norm_scale, (p.y - ref_pt.y) / norm_scale)
}

/// Evaluate all monomial basis functions for a normalized point.
#[inline]
fn evaluate_basis(u: f64, v: f64, terms: &[(usize, usize)], basis: &mut [f64]) {
    for (j, &(p, q)) in terms.iter().enumerate() {
        basis[j] = monomial(u, v, p, q);
    }
}

/// Compute average distance from a set of points to a reference point.
fn avg_distance(points: &[DVec2], ref_pt: DVec2) -> f64 {
    let sum: f64 = points.iter().map(|p| (*p - ref_pt).length()).sum();
    let avg = sum / points.len() as f64;
    if avg > 1e-10 { avg } else { 1.0 }
}

// ============================================================================
// Linear algebra: normal equations and solvers
// ============================================================================

/// Solve the SIP normal equations using only the masked-in points.
fn solve_masked(
    points: &[DVec2],
    targets_u: &[f64],
    targets_v: &[f64],
    mask: &[bool],
    ref_pt: DVec2,
    norm_scale: f64,
    terms: &[(usize, usize)],
) -> Option<(ArrayVec<f64, MAX_TERMS>, ArrayVec<f64, MAX_TERMS>)> {
    let (ata, atb_u, atb_v) = build_normal_equations(
        points, targets_u, targets_v, mask, ref_pt, norm_scale, terms,
    );
    let n_terms = terms.len();
    let coeffs_u = solve_cholesky(&ata, &atb_u, n_terms)?;
    let coeffs_v = solve_cholesky(&ata, &atb_v, n_terms)?;
    Some((coeffs_u, coeffs_v))
}

/// Build normal equations A^T*A and A^T*b from point/target pairs (masked).
fn build_normal_equations(
    points: &[DVec2],
    targets_u: &[f64],
    targets_v: &[f64],
    mask: &[bool],
    ref_pt: DVec2,
    norm_scale: f64,
    terms: &[(usize, usize)],
) -> ([f64; MAX_ATA], [f64; MAX_TERMS], [f64; MAX_TERMS]) {
    let n_terms = terms.len();
    let mut ata = [0.0; MAX_ATA];
    let mut atb_u = [0.0; MAX_TERMS];
    let mut atb_v = [0.0; MAX_TERMS];
    let mut basis = [0.0; MAX_TERMS];

    for (i, point) in points.iter().enumerate() {
        if !mask[i] {
            continue;
        }
        let (u, v) = normalize_point(*point, ref_pt, norm_scale);
        evaluate_basis(u, v, terms, &mut basis[..n_terms]);

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

    (ata, atb_u, atb_v)
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
    if diag_min < super::SINGULAR_THRESHOLD || (diag_max / diag_min) > 1e5 {
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

        if max_val < super::SINGULAR_THRESHOLD {
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
