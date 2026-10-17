//! FWHM conversions of the Gaussian and Moffat profiles.

/// The FWHM of a Gaussian per unit of its sigma: `2·√(2·ln 2)`.
///
/// `f64` is the canonical form, and [`FWHM_PER_SIGMA_F32`] is cast from it, so the two precisions
/// cannot round apart. `sqrt` is not `const`, so the digits are written out; a test holds them to
/// the expression.
pub(crate) const FWHM_PER_SIGMA: f64 = 2.354_820_045_030_949_3;

/// [`FWHM_PER_SIGMA`] in the precision the `f32` paths multiply in.
const FWHM_PER_SIGMA_F32: f32 = FWHM_PER_SIGMA as f32;

/// Convert FWHM to Gaussian sigma.
#[inline]
pub(crate) const fn to_sigma(fwhm: f32) -> f32 {
    fwhm / FWHM_PER_SIGMA_F32
}

/// Convert Gaussian sigma to FWHM.
#[inline]
pub(crate) const fn from_sigma(sigma: f32) -> f32 {
    sigma * FWHM_PER_SIGMA_F32
}

/// The FWHM of a 2-D Gaussian whose covariance has determinant `det` (px⁴): that of the circle of
/// equal area, whose σ is the geometric mean of the principal ones, `√(σ₁·σ₂) = det^¼`. The one
/// FWHM the moments and both profile fits report, so an elongated star reads the same width
/// whichever measured it.
pub(crate) fn equal_area(det: f64) -> f32 {
    from_sigma(det.max(0.0).sqrt().sqrt() as f32)
}

/// The FWHM of a Moffat profile of `beta` per unit of its α: `2·√(2^(1/β) − 1)`.
pub(crate) fn per_alpha(beta: f64) -> f64 {
    2.0 * (2f64.powf(1.0 / beta) - 1.0).sqrt()
}

/// The FWHM of a Moffat profile, `α·`[`per_alpha`]`(β)`.
pub(crate) fn from_alpha_beta(alpha: f64, beta: f64) -> f64 {
    alpha * per_alpha(beta)
}

/// The Moffat α of a profile with this FWHM and `beta`: the inverse of [`from_alpha_beta`].
pub(crate) fn to_alpha(fwhm: f64, beta: f64) -> f64 {
    fwhm / per_alpha(beta)
}

#[cfg(test)]
mod tests;
