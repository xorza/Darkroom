//! [`DrizzleSampling`]: how a drizzle samples each frame onto its output grid — the drop, its
//! scale and its size, or a point, or a Lanczos resampling — each case holding only the values it
//! reads.

use crate::config_field::ConfigField;
use crate::error::InvalidConfigField;

/// How a drizzle samples each frame onto its output grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrizzleSampling {
    /// Each input pixel shrunk by `pixfrac`, dropped onto a grid `scale` times finer in `shape`.
    Drops {
        shape: DropShape,
        scale: DrizzleScale,
        pixfrac: Pixfrac,
    },
    /// Each input pixel's value at the one output pixel its centre lands in, onto a grid `scale`
    /// times finer: the fastest, and it needs very good dithering.
    Point { scale: DrizzleScale },
    /// A Lanczos-3 interpolation of the frame at each output pixel, on the input's own grid: a
    /// resampling rather than a drizzle, and the slowest. Its weights go negative, so it rings at
    /// sharp edges as any Lanczos resampling does.
    Lanczos,
}

/// The shape a drop spreads its flux in over the output grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DropShape {
    /// The exact overlap area of each drop's transformed quadrilateral with every output pixel,
    /// by STScI's `boxer` / `sgarea` edge integration. Correct for any transform, rotation and
    /// shear included. Default, as in DrizzlePac, PixInsight and Siril.
    #[default]
    Square,
    /// An axis-aligned rectangle centred on the transformed pixel centre: exact only at rotations
    /// of 0°, 90° and 180°. (Named "turbo" in `STScI` `DrizzlePac`; "square" there integrates the
    /// transformed polygon's edges, as [`Self::Square`] does.)
    Turbo,
    /// A Gaussian whose FWHM is the drop size, `pixfrac·scale` output pixels, as in `STScI`
    /// drizzle. Smoother output, slight flux redistribution.
    Gaussian,
}

/// How many output pixels an input pixel spans on each side: finite and positive, by
/// [`Self::checked`]. Common values: 1.5, 2, 3.
///
/// The output keeps the input's surface brightness, value per input-pixel area: a flat field of 1
/// drizzles to 1 at any scale, so the image total is `scale²` times the input's. Divide by
/// `scale²` for flux per output pixel, DrizzlePac's convention.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrizzleScale(f64);

/// The side of a drop as a share of its input pixel's: in `(0, 1]`, by [`Self::checked`]. 1 is
/// shift-and-add, 0.8 suits 4-point dithered data, 0.5 needs good dithering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pixfrac(f64);

impl DrizzleSampling {
    /// How many output pixels an input pixel spans on each side: 1 for a Lanczos resampling.
    pub const fn scale(self) -> f64 {
        match self {
            Self::Drops { scale, .. } | Self::Point { scale } => scale.0,
            Self::Lanczos => 1.0,
        }
    }
}

impl Default for DrizzleSampling {
    fn default() -> Self {
        Self::Drops {
            shape: DropShape::Square,
            scale: DrizzleScale(2.0),
            pixfrac: Pixfrac(0.8),
        }
    }
}

impl DrizzleScale {
    /// The scale `scale`.
    ///
    /// # Panics
    /// When [`Self::checked`] refuses it.
    pub fn new(scale: f64) -> Self {
        Self::checked(scale).unwrap_or_else(|invalid| panic!("{invalid}"))
    }

    /// [`Self::new`], or why `scale` is none.
    ///
    /// # Errors
    ///
    /// A scale that is not finite and positive.
    pub fn checked(scale: f64) -> Result<Self, InvalidConfigField> {
        ConfigField::new("drizzle scale", scale)
            .finite_and("finite and positive", |value| value > 0.0)?;
        Ok(Self(scale))
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

impl Pixfrac {
    /// The pixel fraction `pixfrac`.
    ///
    /// # Panics
    /// When [`Self::checked`] refuses it.
    pub fn new(pixfrac: f64) -> Self {
        Self::checked(pixfrac).unwrap_or_else(|invalid| panic!("{invalid}"))
    }

    /// [`Self::new`], or why `pixfrac` is none.
    ///
    /// # Errors
    ///
    /// A fraction that is not finite and in `(0, 1]`.
    pub fn checked(pixfrac: f64) -> Result<Self, InvalidConfigField> {
        ConfigField::new("drizzle pixfrac", pixfrac)
            .finite_and("finite and in (0, 1]", |value| value > 0.0 && value <= 1.0)?;
        Ok(Self(pixfrac))
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use crate::drizzle::drizzle_sampling::{DrizzleSampling, DrizzleScale, DropShape, Pixfrac};

    /// A scale is finite and positive, a pixfrac finite in `(0, 1]`: each end of each range, and
    /// the values past it, refused by the field they break.
    #[test]
    fn the_scale_and_the_pixfrac_are_checked_where_they_are_made() {
        for scale in [0.5, 1.0, 1.5] {
            assert_eq!(DrizzleScale::checked(scale).unwrap().get(), scale);
        }
        for scale in [0.0, -1.0, f64::INFINITY, f64::NAN] {
            assert_eq!(
                DrizzleScale::checked(scale).unwrap_err().field,
                "drizzle scale"
            );
        }
        for pixfrac in [f64::MIN_POSITIVE, 0.8, 1.0] {
            assert_eq!(Pixfrac::checked(pixfrac).unwrap().get(), pixfrac);
        }
        for pixfrac in [0.0, 1.0 + f64::EPSILON, f64::NAN] {
            assert_eq!(
                Pixfrac::checked(pixfrac).unwrap_err().field,
                "drizzle pixfrac"
            );
        }
        // A point and a drop both state their scale; a Lanczos resampling stays on the input's
        // grid.
        let scale = DrizzleScale::new(1.5);
        assert_eq!(DrizzleSampling::Point { scale }.scale(), 1.5);
        assert_eq!(
            DrizzleSampling::Drops {
                shape: DropShape::Gaussian,
                scale,
                pixfrac: Pixfrac::new(0.5),
            }
            .scale(),
            1.5
        );
        assert_eq!(DrizzleSampling::Lanczos.scale(), 1.0);
    }
}
