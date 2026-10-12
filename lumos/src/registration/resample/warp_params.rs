//! [`WarpParams`]: how a warp resamples a frame, and the kernel it resamples with.

use crate::error::InvalidConfigField;

/// Interpolation method for image resampling.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InterpolationMethod {
    /// Nearest neighbor - fastest, lowest quality
    Nearest,
    /// Bilinear interpolation - fast, reasonable quality
    Bilinear,
    /// Bicubic interpolation - good quality
    Bicubic,
    /// Lanczos-2 (4x4 kernel) - high quality
    Lanczos2,
    /// Lanczos-3 (6×6 taps), the default
    #[default]
    Lanczos3,
    /// Lanczos-4 (8×8 taps): a sharper frequency cutoff than Lanczos-3, at more ringing and cost
    Lanczos4,
}

impl InterpolationMethod {
    /// Every method, from the cheapest to the widest kernel.
    pub const ALL: [Self; 6] = [
        Self::Nearest,
        Self::Bilinear,
        Self::Bicubic,
        Self::Lanczos2,
        Self::Lanczos3,
        Self::Lanczos4,
    ];
}

/// PixInsight's default ringing clamp threshold (PCL's `__PCL_LANCZOS_CLAMPING_THRESHOLD`).
const DEFAULT_CLAMPING_THRESHOLD: f32 = 0.3;

/// Configuration for inverse-mapped image resampling.
#[derive(Debug, Clone, Copy)]
pub struct WarpParams {
    /// Resampling kernel.
    pub method: InterpolationMethod,
    /// Fill for source positions outside the closed source pixel footprint.
    ///
    /// Positions inside the footprint with partial kernel support are reconstructed from real
    /// source pixels only.
    pub border_value: f32,
    /// The ringing clamp of a kernel with negative lobes (Bicubic and the Lanczos family), as
    /// PixInsight's: where the negative lobes take more than this share of what the positive lobes
    /// carry, their weight is cut back smoothly, and to nothing once they would cancel it. In
    /// `[0, 1]`, lower clamps harder; `None` turns the clamp off. Default 0.3.
    pub clamping_threshold: Option<f32>,
}

impl Default for WarpParams {
    fn default() -> Self {
        Self {
            method: InterpolationMethod::default(),
            border_value: 0.0,
            clamping_threshold: Some(DEFAULT_CLAMPING_THRESHOLD),
        }
    }
}

impl WarpParams {
    pub(crate) fn validate(self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::finite_only("warp border_value", self.border_value)?;
        if let Some(threshold) = self.clamping_threshold {
            InvalidConfigField::check(
                (0.0..=1.0).contains(&threshold),
                "warp clamping_threshold",
                "between 0 and 1",
                f64::from(threshold),
            )?;
        }
        Ok(())
    }
}
