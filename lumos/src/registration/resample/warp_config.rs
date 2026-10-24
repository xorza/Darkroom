//! [`WarpConfig`]: how a warp resamples a frame, and the kernel it resamples with.

use crate::config_field::ConfigField;
use crate::error::InvalidConfigField;
use crate::image::linear_image::LinearImage;
use crate::registration::resample::source_image::SourceImage;
use crate::registration::resample::{Warp, WarpResult, WarpScratch};
use crate::registration::warp_transform::WarpTransform;

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

/// The share of what a kernel's positive lobes carry that its negative lobes may take before the
/// ringing clamp acts, in `[0, 1]`: lower clamps harder. See [`WarpConfig::clamping_threshold`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClampingThreshold(f32);

impl ClampingThreshold {
    /// PixInsight's default (PCL's `__PCL_LANCZOS_CLAMPING_THRESHOLD`).
    pub const DEFAULT: Self = Self(0.3);

    /// `threshold` as a clamping threshold.
    ///
    /// # Errors
    ///
    /// [`InvalidConfigField`] when `threshold` is not finite or lies outside `[0, 1]`: a NaN would
    /// turn nearly every clamped sample into NaN, and a negative one clamp every pixel.
    pub fn new(threshold: f32) -> Result<Self, InvalidConfigField> {
        ConfigField::new("warp clamping_threshold", threshold)
            .finite_and("between 0 and 1", |threshold| {
                (0.0..=1.0).contains(&threshold)
            })?;
        Ok(Self(threshold))
    }

    /// The threshold.
    pub const fn get(self) -> f32 {
        self.0
    }
}

/// Configuration for inverse-mapped image resampling.
#[derive(Debug, Clone, Copy)]
pub struct WarpConfig {
    /// Resampling kernel.
    pub method: InterpolationMethod,
    /// Fill for source positions outside the closed source pixel footprint.
    ///
    /// Positions inside the footprint with partial kernel support are reconstructed from real
    /// source pixels only.
    pub border_value: f32,
    /// The ringing clamp of a kernel with negative lobes (Bicubic and the Lanczos family), as
    /// PixInsight's: where the negative lobes take more than this share of what the positive lobes
    /// carry, their weight is cut back smoothly, and to nothing once they would cancel it. `None`
    /// turns the clamp off. Default [`ClampingThreshold::DEFAULT`].
    pub clamping_threshold: Option<ClampingThreshold>,
}

impl Default for WarpConfig {
    fn default() -> Self {
        Self {
            method: InterpolationMethod::default(),
            border_value: 0.0,
            clamping_threshold: Some(ClampingThreshold::DEFAULT),
        }
    }
}

impl WarpConfig {
    /// `image` warped by `transform` onto the reference frame's grid, resampled by these
    /// parameters.
    ///
    /// Use `result.warp_transform()` to obtain the transform from a `RegistrationResult`, or
    /// `WarpTransform::new(transform)` for a plain transform.
    ///
    /// The output has the same dimensions and metadata as `image`; every output pixel is produced
    /// by inverse-mapping, so no input pixels are carried over. Returns a [`WarpResult`] carrying
    /// the aligned image and its quality maps (see that type).
    ///
    /// The maps are always produced: the pipeline's only caller needs both, so there is no
    /// unconditional cost to skip. They are geometry, not pixels, so each row's maps are written
    /// once, from the same source positions every channel of that row samples at.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use lumos::{LinearImage, RegistrationConfig, Star, WarpConfig};
    ///
    /// # fn example(ref_stars: &[Star], target_stars: &[Star], target_image: &LinearImage)
    /// # -> Result<(), Box<dyn std::error::Error>> {
    /// let result = RegistrationConfig::default().registrar()?.register(ref_stars, target_stars)?;
    /// let aligned = WarpConfig::default()
    ///     .warp(target_image, result.warp_transform())
    ///     .image;
    /// # Ok(())
    /// # }
    /// ```
    pub fn warp(self, image: &LinearImage, transform: &WarpTransform) -> WarpResult {
        let mut result = WarpResult::new(image.dimensions());
        let warp = Warp {
            transform,
            params: self,
        };
        result.warp_into(&SourceImage::of(image), warp, &WarpScratch::default());
        result
    }

    /// The border value is finite; the threshold is checked when it is made.
    pub(crate) fn validate(self) -> Result<(), InvalidConfigField> {
        ConfigField::new("warp border_value", self.border_value).finite()
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use imaginarium::Buffer2;

    use crate::image::channel_layout::ChannelLayout;
    use crate::image::image_dimensions::ImageDimensions;
    use crate::image::linear_image::LinearImage;
    use crate::registration::resample::warp_config::{InterpolationMethod, WarpConfig};
    use crate::registration::warp_transform::WarpTransform;

    impl WarpConfig {
        /// One plane warped on its own by `transform` — the shape the kernel tests and the plane
        /// benches compare.
        pub(crate) fn warp_plane(
            self,
            input: &Buffer2<f32>,
            transform: &WarpTransform,
        ) -> Buffer2<f32> {
            let image = LinearImage::from_interleaved(
                ImageDimensions::new((input.width(), input.height()), ChannelLayout::Mono),
                input.pixels().to_vec(),
            );
            let warped = self.warp(&image, transform);
            Buffer2::new(
                input.width(),
                input.height(),
                warped.image.channel(0).pixels().to_vec(),
            )
        }

        /// The default parameters with `method` as the kernel.
        pub(crate) fn with_method(method: InterpolationMethod) -> Self {
            Self {
                method,
                ..Self::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::registration::resample::warp_config::{
        ClampingThreshold, InterpolationMethod, WarpConfig,
    };

    #[test]
    fn warp_params_defaults() {
        let default = WarpConfig::default();
        assert_eq!(default.method, InterpolationMethod::Lanczos3);
        assert_eq!(default.border_value, 0.0);
        assert_eq!(default.clamping_threshold, Some(ClampingThreshold::DEFAULT));
        assert_eq!(ClampingThreshold::DEFAULT.get(), 0.3);
        // Both ends of the range are thresholds PixInsight accepts, and `None` is the clamp off;
        // outside it, and NaN, no threshold is made.
        for refused in [1.5, -0.1, f32::NAN] {
            assert_eq!(
                ClampingThreshold::new(refused).unwrap_err().field,
                "warp clamping_threshold"
            );
        }
        for clamping_threshold in [
            Some(ClampingThreshold::new(0.0).unwrap()),
            Some(ClampingThreshold::new(1.0).unwrap()),
            None,
        ] {
            WarpConfig {
                clamping_threshold,
                ..Default::default()
            }
            .validate()
            .unwrap();
        }
        let nan_border = WarpConfig {
            border_value: f32::NAN,
            ..Default::default()
        };
        assert_eq!(
            nan_border.validate().unwrap_err().field,
            "warp border_value"
        );
    }
}
