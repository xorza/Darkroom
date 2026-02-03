//! Fast power functions for fixed exponents.
//!
//! Provides specialized implementations for `u^(neg_beta)` where `neg_beta` is negative,
//! optimized for common half-integer exponents used in Moffat profile fitting.
//!
//! For common half-integer beta values (1.0, 1.5, 2.0, ..., 4.5), uses exact arithmetic
//! (multiply + sqrt) instead of transcendentals. Falls back to `exp(neg_beta * ln(u))`
//! for arbitrary values, which is still faster than `powf` because it skips NaN/sign/edge-case
//! handling that doesn't apply when u > 1.

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

/// Compute `u^(neg_beta)` efficiently where `neg_beta` is negative.
///
/// For common half-integer beta values (1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5),
/// uses exact arithmetic (multiply + sqrt) instead of transcendentals.
/// Falls back to `exp(neg_beta * ln(u))` for arbitrary beta, which is still
/// faster than `powf` because it skips NaN/sign/edge-case handling that doesn't
/// apply here (u is always > 1.0 in the Moffat profile).
#[inline]
pub fn fast_pow_neg_beta(u: f32, neg_beta: f32) -> f32 {
    // Match on common half-integer values of beta (neg_beta = -beta).
    // Multiply by 2 and round to get the half-integer class, but only use
    // the specialized path if beta is actually within epsilon of a half-integer.
    let beta = -neg_beta;
    let twice_beta_f = beta * 2.0;
    let twice_beta = twice_beta_f.round() as i32;

    // Only use specialized path if beta is actually a half-integer (within f32 precision)
    if (twice_beta_f - twice_beta as f32).abs() > 0.01 {
        return (neg_beta * u.ln()).exp();
    }

    match twice_beta {
        // beta = 1.0: u^(-1) = 1/u
        2 => 1.0 / u,
        // beta = 1.5: u^(-1.5) = 1/(u*sqrt(u))
        3 => {
            let sqrt_u = u.sqrt();
            1.0 / (u * sqrt_u)
        }
        // beta = 2.0: u^(-2) = 1/(u*u)
        4 => {
            let u2 = u * u;
            1.0 / u2
        }
        // beta = 2.5: u^(-2.5) = 1/(u*u*sqrt(u))
        5 => {
            let u2 = u * u;
            let sqrt_u = u.sqrt();
            1.0 / (u2 * sqrt_u)
        }
        // beta = 3.0: u^(-3) = 1/(u*u*u)
        6 => {
            let u3 = u * u * u;
            1.0 / u3
        }
        // beta = 3.5: u^(-3.5) = 1/(u*u*u*sqrt(u))
        7 => {
            let u3 = u * u * u;
            let sqrt_u = u.sqrt();
            1.0 / (u3 * sqrt_u)
        }
        // beta = 4.0: u^(-4) = 1/(u*u*u*u)
        8 => {
            let u2 = u * u;
            1.0 / (u2 * u2)
        }
        // beta = 4.5: u^(-4.5) = 1/(u*u*u*u*sqrt(u))
        9 => {
            let u2 = u * u;
            let sqrt_u = u.sqrt();
            1.0 / (u2 * u2 * sqrt_u)
        }
        // Arbitrary beta: use exp(neg_beta * ln(u))
        _ => (neg_beta * u.ln()).exp(),
    }
}
