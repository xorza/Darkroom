//! [`StarNoise`]: the noise of one star's pixels, by the CCD equation.

/// The variance of a pixel of one star's stamp at `signal` above the local sky (Merline & Howell
/// 1995): `σ_B² + max(signal, 0)/G`, with `σ_B` the background's σ measured on the frame and `G`
/// the electrons per unit of the samples, when known. The measured σ already holds the read noise,
/// the dark current and the sky's photons; only the star's own photons are added.
///
/// In f64, which holds the square of a σ at the frame's floor in any domain.
#[derive(Debug, Clone, Copy)]
pub(super) struct StarNoise {
    pub(super) background_sigma: f64,
    pub(super) electrons_per_unit: Option<f64>,
}

impl StarNoise {
    pub(super) fn variance(self, signal: f64) -> f64 {
        let background = self.background_sigma * self.background_sigma;
        match self.electrons_per_unit {
            Some(electrons) => background + signal.max(0.0) / electrons,
            None => background,
        }
    }

    /// The SNR of `flux` summed over `pixels`, its sky measured from `sky_samples` of them when
    /// it was: `F / √(F/G + n·σ_B²·(1 + n/n_B))`, the last factor the error the sky's own estimate
    /// carries into every summed pixel alike (Merline & Howell). A sky from the global map, measured
    /// from thousands of samples per tile, adds no term.
    pub(super) fn snr(self, flux: f64, pixels: usize, sky_samples: Option<usize>) -> f64 {
        let n = pixels as f64;
        let sky_error = sky_samples.map_or(0.0, |samples| n / samples as f64);
        let background = n * self.background_sigma * self.background_sigma * (1.0 + sky_error);
        let source = self
            .electrons_per_unit
            .map_or(0.0, |electrons| flux.max(0.0) / electrons);
        flux / (background + source).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use crate::star_detection::centroid::star_noise::StarNoise;

    /// Flux 2 over 4 pixels of σ 0.02: background-limited, 2/√(4·0.0004) = 50. With 1000 electrons
    /// per unit the source adds 2/1000: 2/√0.0036 = 33.33. Its sky from 16 samples raises the
    /// background term by 1 + 4/16: 2/√(0.002 + 0.002) = 31.62. A pixel's variance at signal 0.5
    /// is 0.0004 + 0.0005, and below the sky the background's alone.
    #[test]
    fn the_ccd_equation_counts_each_term_once() {
        let background = StarNoise {
            background_sigma: 0.02,
            electrons_per_unit: None,
        };
        assert!((background.snr(2.0, 4, None) - 50.0).abs() < 1e-12);
        let gain = StarNoise {
            electrons_per_unit: Some(1000.0),
            ..background
        };
        assert!((gain.snr(2.0, 4, None) - 2.0 / 0.0036f64.sqrt()).abs() < 1e-12);
        assert!((gain.snr(2.0, 4, Some(16)) - 2.0 / 0.004f64.sqrt()).abs() < 1e-12);
        assert!((gain.variance(0.5) - 0.0009).abs() < 1e-15);
        assert!((gain.variance(-0.5) - 0.0004).abs() < 1e-15);
    }
}
