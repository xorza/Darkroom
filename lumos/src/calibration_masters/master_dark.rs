//! [`MasterDark`]: a stacked dark, and whether it still holds the bias.

use crate::io::image::cfa::CfaImage;

/// Whether a master dark still holds the sensor's bias.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DarkBias {
    /// The dark as stacked: bias and thermal signal together. It calibrates only a light of its
    /// own exposure.
    Included,
    /// The bias subtracted: the thermal signal alone, which scales with exposure, so it calibrates
    /// a light of another exposure by the ratio of the two (after the bias).
    Removed,
}

/// A master dark, with its exposure and temperature in its metadata.
#[derive(Debug)]
pub(crate) struct MasterDark {
    pub(crate) image: CfaImage,
    pub(crate) bias: DarkBias,
}

impl MasterDark {
    pub(crate) const fn exposure(&self) -> Option<f64> {
        self.image.metadata.exposure_time
    }

    pub(crate) const fn temperature(&self) -> Option<f64> {
        self.image.metadata.ccd_temp
    }
}
