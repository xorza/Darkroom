//! Matched-filter FWHM selection and estimation settings.

use crate::config_field::ConfigField;
use crate::error::InvalidConfigField;
use crate::star_detection::convolution::filter_kernels::{KernelShape, MatchedFilter};
use crate::star_detection::convolution::{MAX_FWHM, MIN_FWHM};

/// Where the matched filter's FWHM comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FwhmMode {
    /// This FWHM, in pixels: the PSF's, before the pixel integrates it.
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
            ConfigField::new(
                match mode {
                    FwhmMode::Fixed(_) => "fwhm Fixed",
                    FwhmMode::Auto { .. } => "fwhm Auto fallback",
                },
                mode.seed(),
            )
            .finite_and("finite, positive and at most 20 px", |value| {
                value > 0.0 && value <= f64::from(MAX_FWHM)
            })?;
        }
        ConfigField::count("fwhm min_stars", self.min_stars)
            .must_be("at least 5", self.min_stars >= 5)?;
        ConfigField::new("fwhm estimation_sigma_factor", self.estimation_sigma_factor)
            .finite_and("finite and at least 1", |value| value >= 1.0)?;
        ConfigField::new("psf_axis_ratio", self.psf_axis_ratio)
            .finite_and("finite and in (0, 1]", |value| value > 0.0 && value <= 1.0)?;
        ConfigField::new("psf_angle", self.psf_angle).finite()?;
        // The narrowest filter this config can build: the fixed FWHM, or, estimated, the narrowest
        // star the estimate keeps or its fallback.
        let narrowest = match self.mode {
            None => None,
            Some(FwhmMode::Fixed(fwhm)) => Some(fwhm),
            Some(FwhmMode::Auto { fallback }) => Some(fallback.min(MIN_FWHM)),
        };
        if let Some(fwhm) = narrowest
            && let KernelShape::Elliptical(ellipse) = KernelShape::of(self.filter_at(fwhm))
        {
            ConfigField::new("psf_axis_ratio", self.psf_axis_ratio).must_be(
                "wide enough off the pixel axes, at the narrowest FWHM, for a star: the matched \
                 filter's pixel means resolve σ down to 0.01 px across them",
                ellipse.pixel_rule().is_some(),
            )?;
        }
        Ok(())
    }
}
