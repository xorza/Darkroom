//! Matched-filter FWHM selection and estimation settings.

use crate::error::InvalidConfigField;

/// Where the matched filter's FWHM comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FwhmMode {
    /// This FWHM, in pixels.
    Fixed(f32),
    /// Measured from a first pass of bright stars, which `fallback` seeds and which falls back to
    /// it when too few stars pass.
    Auto {
        /// The seed of the measurement and the FWHM when it cannot be made, in pixels.
        fallback: f32,
    },
}

impl FwhmMode {
    /// The FWHM this mode starts from: the fixed one, or the estimate's seed.
    pub const fn seed(self) -> f32 {
        match self {
            Self::Fixed(fwhm) | Self::Auto { fallback: fwhm } => fwhm,
        }
    }
}

/// Configuration for selecting or estimating the matched-filter FWHM.
#[derive(Debug, Clone)]
pub struct FwhmConfig {
    /// The matched filter's FWHM, or `None` to threshold the residual without one.
    pub mode: Option<FwhmMode>,
    /// Minimum first-pass stars required to accept an estimate.
    pub min_stars: usize,
    /// Multiplier applied to the detection threshold during the first pass.
    pub estimation_sigma_factor: f32,
    /// Minor-to-major axis ratio of the matched filter's PSF.
    pub psf_axis_ratio: f32,
    /// Angle of the matched filter's PSF, in radians.
    pub psf_angle: f32,
}

/// The matched filter's PSF: a Gaussian of `fwhm` along its major axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MatchedFilter {
    pub(crate) fwhm: f32,
    pub(crate) axis_ratio: f32,
    pub(crate) angle: f32,
}

impl Default for FwhmConfig {
    fn default() -> Self {
        Self {
            mode: Some(FwhmMode::Fixed(4.0)),
            min_stars: 10,
            estimation_sigma_factor: 2.0,
            psf_axis_ratio: 1.0,
            psf_angle: 0.0,
        }
    }
}

impl FwhmConfig {
    /// The matched filter at `fwhm`, in this config's shape.
    pub(crate) const fn filter_at(&self, fwhm: f32) -> MatchedFilter {
        MatchedFilter {
            fwhm,
            axis_ratio: self.psf_axis_ratio,
            angle: self.psf_angle,
        }
    }

    pub(super) fn validate(&self) -> Result<(), InvalidConfigField> {
        if let Some(mode) = self.mode {
            InvalidConfigField::finite(
                match mode {
                    FwhmMode::Fixed(_) => "fwhm Fixed",
                    FwhmMode::Auto { .. } => "fwhm Auto fallback",
                },
                "finite and positive",
                mode.seed(),
                |value| value > 0.0,
            )?;
        }
        InvalidConfigField::check(
            self.min_stars >= 5,
            "fwhm min_stars",
            "at least 5",
            self.min_stars as f64,
        )?;
        InvalidConfigField::finite(
            "fwhm estimation_sigma_factor",
            "finite and at least 1",
            self.estimation_sigma_factor,
            |value| value >= 1.0,
        )?;
        InvalidConfigField::finite(
            "psf_axis_ratio",
            "finite and in (0, 1]",
            self.psf_axis_ratio,
            |value| value > 0.0 && value <= 1.0,
        )?;
        InvalidConfigField::finite_only("psf_angle", self.psf_angle)?;
        Ok(())
    }
}
