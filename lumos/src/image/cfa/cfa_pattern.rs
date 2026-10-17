//! [`CfaPattern`]: the phase of a 2×2 Bayer filter.

use serde::{Deserialize, Serialize};

use crate::image::cfa::cfa_color::CfaColor;
use crate::image::cfa::demosaic::mosaic_view::MosaicPattern;
use crate::math::vec2us::Vec2us;

/// Bayer CFA (Color Filter Array) pattern.
/// Represents the 2x2 pattern of color filters on the sensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CfaPattern {
    /// RGGB: Red at (0,0), Green at (0,1) and (1,0), Blue at (1,1)
    Rggb,
    /// BGGR: Blue at (0,0), Green at (0,1) and (1,0), Red at (1,1)
    Bggr,
    /// GRBG: Green at (0,0), Red at (0,1), Blue at (1,0), Green at (1,1)
    Grbg,
    /// GBRG: Green at (0,0), Blue at (0,1), Red at (1,0), Green at (1,1)
    Gbrg,
}

impl MosaicPattern for CfaPattern {
    #[inline(always)]
    fn color_at(&self, position: Vec2us) -> CfaColor {
        self.color_at(position)
    }
}

impl CfaPattern {
    /// The four phases, in `BAYERPAT` spelling order.
    pub const ALL: [Self; 4] = [Self::Rggb, Self::Bggr, Self::Grbg, Self::Gbrg];

    /// The FITS `BAYERPAT` value for this phase.
    pub const fn bayerpat(self) -> &'static str {
        match self {
            Self::Rggb => "RGGB",
            Self::Bggr => "BGGR",
            Self::Grbg => "GRBG",
            Self::Gbrg => "GBRG",
        }
    }

    /// The phase a FITS `BAYERPAT` value names, in any case and with blanks around it.
    ///
    /// `"TRUE"` is not a phase. Some writers put a boolean there — "yes, this frame is mosaiced" —
    /// and say nothing about which of the four phases it carries, so it is `None` like any other
    /// value; the FITS loader then takes the phase from `FitsLoadOptions::unstated_bayer_pattern`
    /// or refuses the frame rather than guess.
    pub fn from_bayerpat(s: &str) -> Option<Self> {
        let s = s.trim();
        Self::ALL
            .into_iter()
            .find(|pattern| s.eq_ignore_ascii_case(pattern.bayerpat()))
    }

    /// Flip the pattern vertically (swap rows).
    ///
    /// What a `BOTTOM-UP` FITS needs when its height is **even**: `BAYERPAT` describes the top-down
    /// image, and reversing an even number of rows lands every row on the opposite phase. An odd
    /// height leaves the phases where they were, so the caller must not flip there — see
    /// `Bayerpat::read_cfa`, which is where that parity is checked.
    #[must_use]
    pub const fn flip_vertical(self) -> Self {
        match self {
            Self::Rggb => Self::Gbrg,
            Self::Gbrg => Self::Rggb,
            Self::Bggr => Self::Grbg,
            Self::Grbg => Self::Bggr,
        }
    }

    /// Flip the pattern horizontally (swap columns).
    /// Used when XBAYROFF is odd.
    #[must_use]
    pub const fn flip_horizontal(self) -> Self {
        match self {
            Self::Rggb => Self::Grbg,
            Self::Grbg => Self::Rggb,
            Self::Bggr => Self::Gbrg,
            Self::Gbrg => Self::Bggr,
        }
    }

    /// The color of the photosite at `pos`.
    #[inline(always)]
    pub const fn color_at(&self, pos: Vec2us) -> CfaColor {
        use CfaColor::{Blue, Green, Red};
        let phase = ((pos.y & 1) << 1) | (pos.x & 1);
        match self {
            Self::Rggb => [Red, Green, Green, Blue][phase],
            Self::Bggr => [Blue, Green, Green, Red][phase],
            Self::Grbg => [Green, Red, Blue, Green][phase],
            Self::Gbrg => [Green, Blue, Red, Green][phase],
        }
    }
}
