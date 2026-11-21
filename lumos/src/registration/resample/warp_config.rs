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
    /// Resampling kernel. A position outside the closed source pixel footprint reads 0, its
    /// coverage and confidence 0 and its flags `NO_DATA`, so nothing downstream reads its value;
    /// one inside it with partial kernel support is reconstructed from real source pixels only.
    pub method: InterpolationMethod,
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
            clamping_threshold: Some(ClampingThreshold::DEFAULT),
        }
    }
}

impl WarpConfig {
    /// `image` warped by `transform` onto the reference frame's grid, resampled by these
    /// parameters. An interpolating warp samples the source's surface brightness at each output
    /// pixel and multiplies by no Jacobian: a star's flux scales with the transform's area, and a
    /// frame registered at another plate scale keeps its sky level per pixel, as every stacker's
    /// registration does.
    ///
    /// Use `result.warp()` to obtain the transform from a `RegistrationResult`, or
    /// `WarpTransform::from(transform)` for a plain transform.
    ///
    /// The output has the same dimensions and metadata as `image`; every output pixel is produced
    /// by inverse-mapping, so no input pixels are carried over. Returns a [`WarpResult`] carrying
    /// the aligned image and its quality maps (see that type).
    ///
    /// The maps are always produced, once per row from the source positions every channel of that
    /// row samples at.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use lumos::{LinearImage, Registrar, RegistrationConfig, Star, WarpConfig};
    ///
    /// # fn example(ref_stars: &[Star], target_stars: &[Star], target_image: &LinearImage)
    /// # -> Result<(), Box<dyn std::error::Error>> {
    /// let result = Registrar::new(RegistrationConfig::default()).register(ref_stars, target_stars)?;
    /// let aligned = WarpConfig::default().warp(target_image, result.warp()).image;
    /// # Ok(())
    /// # }
    /// ```
    pub fn warp(self, image: &LinearImage, transform: &WarpTransform) -> WarpResult {
        let mut result = WarpResult::new(image.desc());
        let warp = Warp {
            transform,
            params: self,
        };
        result.warp_into(&SourceImage::of(image), warp, &WarpScratch::default());
        result
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use common::Buffer2;
    use imaginarium::{ChannelLayout, PlanarDesc};

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
                PlanarDesc::new(input.size(), ChannelLayout::Mono),
                input.pixels().to_vec(),
            );
            let warped = self.warp(&image, transform);
            Buffer2::new(input.size(), warped.image.channel(0).pixels().to_vec())
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
        for threshold in [0.0, 1.0] {
            assert_eq!(ClampingThreshold::new(threshold).unwrap().get(), threshold);
        }
    }
}
