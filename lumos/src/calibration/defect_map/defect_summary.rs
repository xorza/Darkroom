//! [`DefectSummary`]: how many pixels a defect map names, in all and per colour.

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

/// Read-only defect statistics of a calibration bundle: the hot and cold pixels in all, and per
/// colour of the sensor's filter, so a colour that lost an unusual share of its photosites shows.
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
    /// Per colour, indexed as [`CfaType::color_at`](crate::CfaType::color_at) numbers them: red,
    /// green and blue, or the one colour of a mono sensor.
    pub colours: Vec<ColourDefects>,
}

impl DefectSummary {
    /// The defective pixels as a percentage of the sensor's.
    pub fn percentage(&self) -> f32 {
        percentage(self.defective_pixels, self.pixels)
    }
}

/// The defects of one colour of the sensor's filter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ColourDefects {
    /// Hot pixels of this colour.
    pub hot: usize,
    /// Cold or dead pixels of this colour.
    pub cold: usize,
    /// Pixels in either class, a pixel in both once.
    pub defective: usize,
    /// The sensor's photosites of this colour.
    pub photosites: usize,
}

impl ColourDefects {
    /// The defective photosites as a percentage of this colour's; `0` for a colour the sensor has
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

/// `12 defective pixels, 0.005%`, and on a colour sensor each colour's share after it:
/// `(R 0.010%, G 0.002%, B 0.008%)`.
impl Display for DefectSummary {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} defective pixels, {:.3}%",
            self.defective_pixels,
            self.percentage()
        )?;
        if let [red, green, blue] = self.colours.as_slice() {
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
