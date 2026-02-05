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

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

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
}

impl Default for SipConfig {
    fn default() -> Self {
        Self {
            order: 3,
            reference_point: None,
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
    }
}

// ---------------------------------------------------------------------------
// Constants and polynomial helpers
// ---------------------------------------------------------------------------

/// Maximum number of polynomial terms (order 5): (5+1)(5+2)/2 - 3 = 18.
const MAX_TERMS: usize = 18;
/// Maximum size of the A^T*A matrix (flattened).
const MAX_ATA: usize = MAX_TERMS * MAX_TERMS;
/// Maximum size of the LU augmented matrix: MAX_TERMS * (MAX_TERMS + 1).
const MAX_AUG: usize = MAX_TERMS * (MAX_TERMS + 1);

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

/// Evaluate a polynomial correction at a normalized point.
fn evaluate_correction(
    p: DVec2,
    reference_point: DVec2,
    norm_scale: f64,
    terms: &[(usize, usize)],
    coeffs_u: &[f64],
    coeffs_v: &[f64],
) -> DVec2 {
    let u = (p.x - reference_point.x) / norm_scale;
    let v = (p.y - reference_point.y) / norm_scale;

    let mut du_norm = 0.0;
    let mut dv_norm = 0.0;
    for (i, &(exp_p, exp_q)) in terms.iter().enumerate() {
        let basis = monomial(u, v, exp_p, exp_q);
        du_norm += coeffs_u[i] * basis;
        dv_norm += coeffs_v[i] * basis;
    }

    DVec2::new(du_norm * norm_scale, dv_norm * norm_scale)
}

/// Compute average distance from a set of points to a reference point.
fn avg_distance(points: &[DVec2], ref_pt: DVec2) -> f64 {
    let sum: f64 = points.iter().map(|p| (*p - ref_pt).length()).sum();
    let avg = sum / points.len() as f64;
    if avg > 1e-10 { avg } else { 1.0 }
}

// ---------------------------------------------------------------------------
// Normal equations and solvers
// ---------------------------------------------------------------------------

/// Build normal equations A^T*A and A^T*b from point/target pairs.
fn build_normal_equations(
    points: &[DVec2],
    targets_u: &[f64],
    targets_v: &[f64],
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
        let u = (point.x - ref_pt.x) / norm_scale;
        let v = (point.y - ref_pt.y) / norm_scale;

        for (j, &(p, q)) in terms.iter().enumerate() {
            basis[j] = monomial(u, v, p, q);
        }

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
#[allow(clippy::needless_range_loop)]
fn solve_symmetric_positive(a: &[f64], b: &[f64], n: usize) -> Option<ArrayVec<f64, MAX_TERMS>> {
    let mut l = [0.0; MAX_ATA];

    for i in 0..n {
        for j in 0..=i {
            let mut sum = 0.0;
            for k in 0..j {
                sum += l[i * n + k] * l[j * n + k];
            }

            if i == j {
                let diag = a[i * n + i] - sum;
                if diag <= 0.0 {
                    return solve_lu(a, b, n);
                }
                l[i * n + j] = diag.sqrt();
            } else {
                l[i * n + j] = (a[i * n + j] - sum) / l[j * n + j];
            }
        }
    }

    let mut y = [0.0; MAX_TERMS];
    for i in 0..n {
        let mut sum = 0.0;
        for j in 0..i {
            sum += l[i * n + j] * y[j];
        }
        y[i] = (b[i] - sum) / l[i * n + i];
    }

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

/// Fallback LU decomposition solver with partial pivoting.
#[allow(clippy::needless_range_loop)]
fn solve_lu(a: &[f64], b: &[f64], n: usize) -> Option<ArrayVec<f64, MAX_TERMS>> {
    let mut aug = [0.0; MAX_AUG];
    for i in 0..n {
        for j in 0..n {
            aug[i * (n + 1) + j] = a[i * n + j];
        }
        aug[i * (n + 1) + n] = b[i];
    }

    for col in 0..n {
        let mut max_row = col;
        let mut max_val = aug[col * (n + 1) + col].abs();
        for row in (col + 1)..n {
            let val = aug[row * (n + 1) + col].abs();
            if val > max_val {
                max_val = val;
                max_row = row;
            }
        }

        if max_val < 1e-12 {
            return None;
        }

        if max_row != col {
            for j in 0..=n {
                let idx_a = col * (n + 1) + j;
                let idx_b = max_row * (n + 1) + j;
                aug.swap(idx_a, idx_b);
            }
        }

        for row in (col + 1)..n {
            let factor = aug[row * (n + 1) + col] / aug[col * (n + 1) + col];
            for j in col..=n {
                aug[row * (n + 1) + j] -= factor * aug[col * (n + 1) + j];
            }
        }
    }

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

// ---------------------------------------------------------------------------
// SipPolynomial
// ---------------------------------------------------------------------------

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

impl SipPolynomial {
    /// Fit SIP polynomial directly from matched point pairs and a transform.
    ///
    /// Given matched inlier positions and a homography, fits a polynomial
    /// correction to minimize residual errors.
    ///
    /// Returns `None` if the system is underdetermined or singular.
    pub fn fit_from_transform(
        ref_points: &[DVec2],
        target_points: &[DVec2],
        transform: &crate::registration::transform::Transform,
        config: &SipConfig,
    ) -> Option<Self> {
        config.validate();

        assert_eq!(
            ref_points.len(),
            target_points.len(),
            "ref_points and target_points must have the same length"
        );

        let n = ref_points.len();
        let terms = term_exponents(config.order);
        if n < terms.len() {
            return None;
        }

        let ref_pt = config.reference_point.unwrap_or_else(|| {
            let sum: DVec2 = ref_points.iter().sum();
            sum / n as f64
        });

        let norm_scale = avg_distance(ref_points, ref_pt);

        // Compute residuals: transform(ref) - target
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

        let (ata, atb_u, atb_v) = build_normal_equations(
            ref_points, &targets_u, &targets_v, ref_pt, norm_scale, &terms,
        );

        let n_terms = terms.len();
        let coeffs_u = solve_symmetric_positive(&ata, &atb_u, n_terms)?;
        let coeffs_v = solve_symmetric_positive(&ata, &atb_v, n_terms)?;

        Some(Self {
            reference_point: ref_pt,
            norm_scale,
            terms,
            coeffs_u,
            coeffs_v,
        })
    }

    /// Apply the SIP correction to a point.
    pub fn correct(&self, p: DVec2) -> DVec2 {
        let correction = evaluate_correction(
            p,
            self.reference_point,
            self.norm_scale,
            &self.terms,
            &self.coeffs_u,
            &self.coeffs_v,
        );
        p + correction
    }

    /// Compute residuals after applying SIP correction.
    ///
    /// For each point, computes `|transform(sip_correct(ref)) - target|`.
    pub fn compute_corrected_residuals(
        &self,
        ref_points: &[DVec2],
        target_points: &[DVec2],
        transform: &crate::registration::transform::Transform,
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
                let p = DVec2::new(x, y);
                let correction = evaluate_correction(
                    p,
                    self.reference_point,
                    self.norm_scale,
                    &self.terms,
                    &self.coeffs_u,
                    &self.coeffs_v,
                );
                max_mag = max_mag.max(correction.length());
                x += grid_spacing;
            }
            y += grid_spacing;
        }
        max_mag
    }
}

#[cfg(test)]
mod tests;
