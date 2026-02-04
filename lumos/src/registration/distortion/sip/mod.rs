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
//!
//! # Usage
//!
//! ```ignore
//! use glam::DVec2;
//! use lumos::registration::distortion::{SipPolynomial, SipConfig};
//!
//! // After RANSAC finds homography + inliers, collect residual vectors
//! let ref_points: Vec<DVec2> = /* inlier reference positions */;
//! let residuals: Vec<DVec2> = /* transform(ref) - target for each inlier */;
//!
//! let config = SipConfig { order: 3, ..Default::default() };
//! let sip = SipPolynomial::fit_residuals(&ref_points, &residuals, &config).unwrap();
//!
//! // Apply correction to a point before the homography
//! let corrected = sip.correct(DVec2::new(100.0, 200.0));
//! ```

use arrayvec::ArrayVec;
use glam::DVec2;

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
    /// Validate configuration.
    pub fn validate(&self) {
        assert!(
            (2..=5).contains(&self.order),
            "SIP order must be 2-5, got {}",
            self.order
        );
    }
}

/// Maximum number of polynomial terms (order 5): (5+1)(5+2)/2 - 3 = 18.
const MAX_TERMS: usize = 18;
/// Maximum size of the A^T*A matrix (flattened).
const MAX_ATA: usize = MAX_TERMS * MAX_TERMS;
/// Maximum size of the LU augmented matrix: MAX_TERMS * (MAX_TERMS + 1).
const MAX_AUG: usize = MAX_TERMS * (MAX_TERMS + 1);

/// Number of polynomial terms for a given order (excluding linear and constant).
/// Terms satisfy: 2 ≤ p+q ≤ order, p ≥ 0, q ≥ 0
fn num_terms(order: usize) -> usize {
    // Total terms with 0 ≤ p+q ≤ order is (order+1)(order+2)/2
    // Minus the 3 terms with p+q < 2 (constant, u, v)
    (order + 1) * (order + 2) / 2 - 3
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
    debug_assert_eq!(terms.len(), num_terms(order));
    terms
}

/// Evaluate a monomial u^p * v^q.
#[inline]
fn monomial(u: f64, v: f64, p: usize, q: usize) -> f64 {
    u.powi(p as i32) * v.powi(q as i32)
}

/// Evaluate a polynomial correction at a normalized point.
///
/// Shared by forward and inverse correction evaluation.
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

/// Build normal equations A^T*A and A^T*b from point/target pairs.
///
/// Points are normalized relative to `ref_pt` by `norm_scale`. Targets
/// are already in normalized space (divided by norm_scale by the caller).
/// Returns `(ata, atb_u, atb_v)` using stack-allocated arrays.
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

/// Compute average distance from a set of points to a reference point.
fn avg_distance(points: &[DVec2], ref_pt: DVec2) -> f64 {
    let sum: f64 = points.iter().map(|p| (*p - ref_pt).length()).sum();
    let avg = sum / points.len() as f64;
    if avg > 1e-10 { avg } else { 1.0 }
}

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
    /// Polynomial order (2-5).
    order: usize,
    /// Reference point (coordinates are relative to this).
    reference_point: DVec2,
    /// Normalization scale: normalized = (pixel - ref_pt) / norm_scale.
    norm_scale: f64,
    /// Precomputed (p, q) exponent pairs for this order.
    terms: ArrayVec<(usize, usize), MAX_TERMS>,
    /// Coefficients for the u-correction polynomial (A_pq) in normalized space.
    coeffs_u: ArrayVec<f64, MAX_TERMS>,
    /// Coefficients for the v-correction polynomial (B_pq) in normalized space.
    coeffs_v: ArrayVec<f64, MAX_TERMS>,
    /// Inverse polynomial order (0 if not computed).
    inv_order: usize,
    /// Normalization scale for inverse polynomial (computed from corrected coordinates).
    inv_norm_scale: f64,
    /// Precomputed (p, q) exponent pairs for inverse order. Empty if not computed.
    inv_terms: ArrayVec<(usize, usize), MAX_TERMS>,
    /// Inverse u-correction coefficients (AP_pq). Empty if not computed.
    inv_coeffs_u: ArrayVec<f64, MAX_TERMS>,
    /// Inverse v-correction coefficients (BP_pq). Empty if not computed.
    inv_coeffs_v: ArrayVec<f64, MAX_TERMS>,
}

impl SipPolynomial {
    /// Fit SIP polynomial to residual vectors after a linear transform.
    ///
    /// Given matched inlier positions in the reference frame and their
    /// residual errors after applying a homography, fits a polynomial
    /// correction to minimize those residuals.
    ///
    /// # Arguments
    /// * `ref_points` - Inlier reference positions (in pixel coordinates)
    /// * `residuals` - Residual vectors: `homography(ref) - target` for each inlier
    /// * `config` - SIP configuration (order, reference point)
    ///
    /// # Returns
    /// Fitted SIP polynomial, or None if the system is underdetermined or singular.
    pub fn fit_residuals(
        ref_points: &[DVec2],
        residuals: &[DVec2],
        config: &SipConfig,
    ) -> Option<Self> {
        config.validate();
        let n = ref_points.len();
        assert_eq!(
            n,
            residuals.len(),
            "ref_points and residuals must have the same length"
        );

        let terms = term_exponents(config.order);

        // Need at least as many points as terms
        if n < terms.len() {
            return None;
        }

        // Compute reference point
        let ref_pt = config.reference_point.unwrap_or_else(|| {
            let sum: DVec2 = ref_points.iter().sum();
            sum / n as f64
        });

        let norm_scale = avg_distance(ref_points, ref_pt);

        // Prepare normalized targets (negated residuals)
        let targets_u: Vec<f64> = residuals.iter().map(|r| -r.x / norm_scale).collect();
        let targets_v: Vec<f64> = residuals.iter().map(|r| -r.y / norm_scale).collect();

        let (ata, atb_u, atb_v) = build_normal_equations(
            ref_points, &targets_u, &targets_v, ref_pt, norm_scale, &terms,
        );

        let n_terms = terms.len();
        let coeffs_u = solve_symmetric_positive(&ata, &atb_u, n_terms)?;
        let coeffs_v = solve_symmetric_positive(&ata, &atb_v, n_terms)?;

        Some(Self {
            order: config.order,
            reference_point: ref_pt,
            norm_scale,
            terms,
            coeffs_u,
            coeffs_v,
            inv_order: 0,
            inv_norm_scale: 0.0,
            inv_terms: ArrayVec::new(),
            inv_coeffs_u: ArrayVec::new(),
            inv_coeffs_v: ArrayVec::new(),
        })
    }

    /// Fit SIP polynomial directly from matched point pairs and a transform.
    ///
    /// Convenience method that computes residuals internally.
    pub fn fit_from_transform(
        ref_points: &[DVec2],
        target_points: &[DVec2],
        transform: &crate::registration::transform::Transform,
        config: &SipConfig,
    ) -> Option<Self> {
        assert_eq!(
            ref_points.len(),
            target_points.len(),
            "ref_points and target_points must have the same length"
        );

        let residuals: Vec<DVec2> = ref_points
            .iter()
            .zip(target_points.iter())
            .map(|(&r, &t)| transform.apply(r) - t)
            .collect();

        Self::fit_residuals(ref_points, &residuals, config)
    }

    /// Apply the SIP correction to a point.
    pub fn correct(&self, p: DVec2) -> DVec2 {
        p + self.correction_at(p)
    }

    /// Apply the SIP correction to multiple points.
    pub fn correct_points(&self, points: &[DVec2]) -> Vec<DVec2> {
        points.iter().map(|&p| self.correct(p)).collect()
    }

    /// Evaluate just the correction vector (du, dv) at a point in pixel space.
    pub fn correction_at(&self, p: DVec2) -> DVec2 {
        evaluate_correction(
            p,
            self.reference_point,
            self.norm_scale,
            &self.terms,
            &self.coeffs_u,
            &self.coeffs_v,
        )
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

    /// Get the polynomial order.
    pub fn order(&self) -> usize {
        self.order
    }

    /// Get the reference point.
    pub fn reference_point(&self) -> DVec2 {
        self.reference_point
    }

    /// Get the u-correction coefficients (A_pq).
    pub fn coeffs_u(&self) -> &[f64] {
        &self.coeffs_u
    }

    /// Get the v-correction coefficients (B_pq).
    pub fn coeffs_v(&self) -> &[f64] {
        &self.coeffs_v
    }

    /// Compute the inverse polynomial by grid-sampling the forward correction.
    ///
    /// Lays a grid over `[0, width] × [0, height]`, applies the forward
    /// correction to each grid point, then fits a polynomial from corrected
    /// coordinates back to original coordinates. This follows the approach
    /// used by astrometry.net and LSST.
    ///
    /// Returns the maximum round-trip error (pixels) over the grid:
    /// `max |inverse_correct(correct(p)) - p|`.
    pub fn compute_inverse(&mut self, width: usize, height: usize) -> f64 {
        let inv_order = (self.order + 1).min(5);
        let grid_side = 20 * (inv_order + 1);

        // Generate grid and apply forward correction
        let step_x = width as f64 / (grid_side - 1) as f64;
        let step_y = height as f64 / (grid_side - 1) as f64;

        let mut originals = Vec::with_capacity(grid_side * grid_side);
        let mut corrected = Vec::with_capacity(grid_side * grid_side);

        for iy in 0..grid_side {
            for ix in 0..grid_side {
                let p = DVec2::new(ix as f64 * step_x, iy as f64 * step_y);
                let c = self.correct(p);
                originals.push(p);
                corrected.push(c);
            }
        }

        let inv_norm_scale = avg_distance(&corrected, self.reference_point);
        let inv_terms = term_exponents(inv_order);

        // Prepare normalized targets: delta = (original - corrected) / inv_norm_scale
        let targets_u: Vec<f64> = originals
            .iter()
            .zip(corrected.iter())
            .map(|(o, c)| (o.x - c.x) / inv_norm_scale)
            .collect();
        let targets_v: Vec<f64> = originals
            .iter()
            .zip(corrected.iter())
            .map(|(o, c)| (o.y - c.y) / inv_norm_scale)
            .collect();

        let (ata, atb_u, atb_v) = build_normal_equations(
            &corrected,
            &targets_u,
            &targets_v,
            self.reference_point,
            inv_norm_scale,
            &inv_terms,
        );

        let n_terms = inv_terms.len();
        let inv_coeffs_u = solve_symmetric_positive(&ata, &atb_u, n_terms)
            .expect("inverse SIP solve failed: singular normal equations on grid data");
        let inv_coeffs_v = solve_symmetric_positive(&ata, &atb_v, n_terms)
            .expect("inverse SIP solve failed: singular normal equations on grid data");

        self.inv_order = inv_order;
        self.inv_norm_scale = inv_norm_scale;
        self.inv_terms = inv_terms;
        self.inv_coeffs_u = inv_coeffs_u;
        self.inv_coeffs_v = inv_coeffs_v;

        // Compute max round-trip error
        corrected
            .iter()
            .zip(originals.iter())
            .map(|(&c, &o)| (self.inverse_correct(c) - o).length())
            .fold(0.0f64, f64::max)
    }

    /// Whether the inverse polynomial has been computed.
    pub fn has_inverse(&self) -> bool {
        !self.inv_coeffs_u.is_empty()
    }

    /// Apply the inverse SIP correction to a single point.
    ///
    /// Given a corrected coordinate, returns the original pixel coordinate.
    /// Panics if `compute_inverse` has not been called.
    pub fn inverse_correct(&self, p: DVec2) -> DVec2 {
        p + self.inverse_correction_at(p)
    }

    /// Evaluate the inverse correction vector at a point.
    ///
    /// Returns the delta to add to a corrected coordinate to recover the
    /// original pixel coordinate. Panics if `compute_inverse` has not been called.
    pub fn inverse_correction_at(&self, p: DVec2) -> DVec2 {
        assert!(
            self.has_inverse(),
            "inverse polynomial not computed; call compute_inverse first"
        );
        evaluate_correction(
            p,
            self.reference_point,
            self.inv_norm_scale,
            &self.inv_terms,
            &self.inv_coeffs_u,
            &self.inv_coeffs_v,
        )
    }

    /// Apply inverse SIP correction to multiple points (allocating).
    ///
    /// Panics if `compute_inverse` has not been called.
    pub fn inverse_correct_points(&self, points: &[DVec2]) -> Vec<DVec2> {
        points.iter().map(|&p| self.inverse_correct(p)).collect()
    }

    /// Get the inverse polynomial order.
    pub fn inv_order(&self) -> usize {
        self.inv_order
    }

    /// Get the inverse u-correction coefficients (AP_pq).
    pub fn inv_coeffs_u(&self) -> &[f64] {
        &self.inv_coeffs_u
    }

    /// Get the inverse v-correction coefficients (BP_pq).
    pub fn inv_coeffs_v(&self) -> &[f64] {
        &self.inv_coeffs_v
    }

    /// Get the maximum correction magnitude across a grid of points.
    pub fn max_correction(&self, width: usize, height: usize, grid_spacing: f64) -> f64 {
        let mut max_mag = 0.0f64;
        let mut y = 0.0;
        while y <= height as f64 {
            let mut x = 0.0;
            while x <= width as f64 {
                let c = self.correction_at(DVec2::new(x, y));
                max_mag = max_mag.max(c.length());
                x += grid_spacing;
            }
            y += grid_spacing;
        }
        max_mag
    }
}

/// Solve a symmetric positive definite system Ax = b using Cholesky decomposition.
///
/// `a` is stored as a flat row-major n×n matrix. All storage is stack-allocated.
#[allow(clippy::needless_range_loop)]
fn solve_symmetric_positive(a: &[f64], b: &[f64], n: usize) -> Option<ArrayVec<f64, MAX_TERMS>> {
    // Cholesky decomposition: A = L * L^T
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
                    // Not positive definite — fall back to LU
                    return solve_lu(a, b, n);
                }
                l[i * n + j] = diag.sqrt();
            } else {
                l[i * n + j] = (a[i * n + j] - sum) / l[j * n + j];
            }
        }
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

/// Fallback LU decomposition solver with partial pivoting.
#[allow(clippy::needless_range_loop)]
fn solve_lu(a: &[f64], b: &[f64], n: usize) -> Option<ArrayVec<f64, MAX_TERMS>> {
    // Create augmented matrix
    let mut aug = [0.0; MAX_AUG];
    for i in 0..n {
        for j in 0..n {
            aug[i * (n + 1) + j] = a[i * n + j];
        }
        aug[i * (n + 1) + n] = b[i];
    }

    // Forward elimination with partial pivoting
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

        if max_val < 1e-12 {
            return None; // Singular
        }

        // Swap rows
        if max_row != col {
            for j in 0..=n {
                let idx_a = col * (n + 1) + j;
                let idx_b = max_row * (n + 1) + j;
                aug.swap(idx_a, idx_b);
            }
        }

        // Eliminate
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

#[cfg(test)]
mod tests;
