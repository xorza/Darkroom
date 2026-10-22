//! [`DefectSummary`]: how many pixels a defect map names, in all and per color.

use std::fmt;
use std::fmt::{Display, Formatter};

use crate::image::cfa::cfa_color::CfaColor;

/// Read-only defect statistics of a calibration bundle: the hot and cold pixels in all, and per
/// color of the sensor's filter, so a color that lost an unusual share of its photosites shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefectSummary {
    /// Hot pixels detected from the master dark.
    pub hot_pixels: usize,
    /// Cold or dead pixels detected from the master flat.
    pub cold_pixels: usize,
    /// Pixels in either class, a pixel in both once.
    pub defective_pixels: usize,
    /// The sensor's pixels.
    pub pixels: usize,
    /// Per color of the sensor's filter.
    pub colors: ColorShares,
}

/// The defects of each color of a sensor's filter: a mono sensor's one, or a mosaic's red, green
/// and blue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorShares {
    Mono(ColorDefects),
    /// Each color's, at its [`CfaColor::idx`].
    Mosaic([ColorDefects; 3]),
}

impl ColorShares {
    /// The defects of `color`; `None` on a mono sensor, which has no filter color.
    pub const fn of(&self, color: CfaColor) -> Option<&ColorDefects> {
        match self {
            Self::Mono(_) => None,
            Self::Mosaic(colors) => Some(&colors[color.idx()]),
        }
    }

    /// The defects of the color in `slot`, as [`CfaType::slot_at`](crate::CfaType::slot_at)
    /// numbers them.
    pub(super) const fn slot_mut(&mut self, slot: usize) -> &mut ColorDefects {
        match self {
            Self::Mono(defects) => defects,
            Self::Mosaic(colors) => &mut colors[slot],
        }
    }
}

impl DefectSummary {
    /// The defective pixels as a percentage of the sensor's.
    pub fn percentage(&self) -> f32 {
        percentage(self.defective_pixels, self.pixels)
    }
}

/// The defects of one color of the sensor's filter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ColorDefects {
    /// Hot pixels of this color.
    pub hot: usize,
    /// Cold or dead pixels of this color.
    pub cold: usize,
    /// Pixels in either class, a pixel in both once.
    pub defective: usize,
    /// The sensor's photosites of this color.
    pub photosites: usize,
}

impl ColorDefects {
    /// The defective photosites as a percentage of this color's; `0` for a color the sensor has
    /// none of.
    pub fn percentage(&self) -> f32 {
        percentage(self.defective, self.photosites)
    }
}

fn percentage(part: usize, whole: usize) -> f32 {
    if whole == 0 {
        0.0
    } else {
        100.0 * part as f32 / whole as f32
    }
}

/// `12 defective pixels, 0.005%`, and on a color sensor each color's share after it:
/// `(R 0.010%, G 0.002%, B 0.008%)`.
impl Display for DefectSummary {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} defective pixels, {:.3}%",
            self.defective_pixels,
            self.percentage()
        )?;
        if let ColorShares::Mosaic([red, green, blue]) = &self.colors {
            write!(
                formatter,
                " (R {:.3}%, G {:.3}%, B {:.3}%)",
                red.percentage(),
                green.percentage(),
                blue.percentage()
            )?;
        }
        Ok(())
    }
}
