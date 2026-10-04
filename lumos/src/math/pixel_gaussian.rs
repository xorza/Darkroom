//! [`PixelGaussian`]: a 1-D Gaussian's mean over a pixel, in closed form.

use crate::math::error_function;

/// The profile `exp(−t²/2σ²)` along one axis, as a pixel records it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PixelGaussian {
    pub(crate) sigma: f64,
}

impl PixelGaussian {
    /// The profile's mean over the pixel `[d − ½, d + ½]`:
    /// `σ·√(π/2)·(erf((d + ½)/(σ√2)) − erf((d − ½)/(σ√2)))`. On one side of the centre the
    /// difference is taken between the complements, which keep their relative precision far out
    /// in the wing; `d` and `−d` give the same value bit for bit.
    pub(crate) fn mean_at(self, d: f64) -> f64 {
        let scale = 1.0 / (self.sigma * std::f64::consts::SQRT_2);
        let (low, high) = ((d - 0.5) * scale, (d + 0.5) * scale);
        let span = if low >= 0.0 {
            error_function::erfc(low) - error_function::erfc(high)
        } else if high <= 0.0 {
            error_function::erfc(-high) - error_function::erfc(-low)
        } else {
            error_function::erf(high) - error_function::erf(low)
        };
        self.sigma * (std::f64::consts::PI / 2.0).sqrt() * span
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::pixel_quadrature::PixelQuadrature;

    /// The closed form against a 16-point quadrature of the profile, which errs under 1e-15 of the
    /// peak at σ 0.5, over the centre and far into the wing at σ 0.5, 1 and 3. Two erfs each 2
    /// ulps off, whose difference cancels by up to a factor 10 here, hold the mean to 1e-14 of
    /// itself; far out, the complements keep that relative precision where a difference of erfs
    /// would lose it, at 8σ near 1e-14 of the peak. Symmetric bit for bit.
    #[test]
    fn the_closed_form_matches_quadrature() {
        let rule = PixelQuadrature::gauss_legendre(16);
        for sigma in [0.5, 1.0, 3.0] {
            let gaussian = PixelGaussian { sigma };
            for d in [0.0, 0.3, 0.5, 1.0, 2.5, 4.0 * sigma, 8.0 * sigma] {
                let quadrature =
                    rule.integrate(d, 0.0, |t, _| (-t * t / (2.0 * sigma * sigma)).exp());
                let closed = gaussian.mean_at(d);
                assert!(
                    (closed - quadrature).abs() <= 1e-14 * quadrature + 1e-15,
                    "σ {sigma}, d {d}: {closed} vs {quadrature}"
                );
                assert_eq!(closed.to_bits(), gaussian.mean_at(-d).to_bits());
            }
        }
    }
}
