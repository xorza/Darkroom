//! Bayer CFA demosaicing module.

use crate::io::raw::demosaic::sensor_layout::SensorLayout;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

pub(crate) mod rcd;
#[cfg(test)]
mod tests;

/// Bayer CFA (Color Filter Array) pattern.
/// Represents the 2x2 pattern of color filters on the sensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

impl CfaPattern {
    /// Parse from FITS BAYERPAT header value (e.g. "RGGB", "BGGR", "GRBG", "GBRG").
    ///
    /// `"TRUE"` is not a pattern. Some writers put a boolean there — "yes, this frame is mosaiced"
    /// — and say nothing about which of the four phases it carries. Reading it as RGGB is a guess
    /// at the most common one, right roughly a quarter of the time and a fully mis-debayered frame
    /// otherwise, so it is warned about rather than resolved silently. Kept because rejecting it
    /// would leave those files unloadable with no way to state the pattern by hand.
    pub fn from_bayerpat(s: &str) -> Option<Self> {
        match s.trim().to_uppercase().as_str() {
            "RGGB" => Some(CfaPattern::Rggb),
            "TRUE" => {
                tracing::warn!(
                    "BAYERPAT is 'TRUE', which states that the frame is mosaiced but not in which \
                     phase; assuming RGGB. A wrong assumption here mis-debayers the frame."
                );
                Some(CfaPattern::Rggb)
            }
            "BGGR" => Some(CfaPattern::Bggr),
            "GRBG" => Some(CfaPattern::Grbg),
            "GBRG" => Some(CfaPattern::Gbrg),
            _ => None,
        }
    }

    /// Parse from LibRaw's `filters` field, which encodes the color at each position of a
    /// repeating pattern — 2x2 for Bayer sensors — two bits per position:
    /// `color_index = (filters >> (((row << 1 & 14) | (col & 1)) << 1)) & 3`
    ///
    /// Color indices: 0=Red, 1=Green, 2=Blue, 3=Green2.
    ///
    /// Returns `None` when the pattern is not a known 2x2 Bayer CFA — X-Trans, monochrome, and
    /// other exotic sensors all land here.
    pub(crate) fn from_filters(filters: u32) -> Option<Self> {
        let color_at =
            |row: u32, col: u32| -> u32 { (filters >> (((row << 1 & 14) | (col & 1)) << 1)) & 3 };

        let c00 = color_at(0, 0);
        let c01 = color_at(0, 1);
        let c10 = color_at(1, 0);
        let c11 = color_at(1, 1);

        let is_red = |c: u32| c == 0;
        // Both green indices count as green.
        let is_green = |c: u32| c == 1 || c == 3;
        let is_blue = |c: u32| c == 2;

        if is_red(c00) && is_green(c01) && is_green(c10) && is_blue(c11) {
            return Some(CfaPattern::Rggb);
        }
        if is_blue(c00) && is_green(c01) && is_green(c10) && is_red(c11) {
            return Some(CfaPattern::Bggr);
        }
        if is_green(c00) && is_red(c01) && is_blue(c10) && is_green(c11) {
            return Some(CfaPattern::Grbg);
        }
        if is_green(c00) && is_blue(c01) && is_red(c10) && is_green(c11) {
            return Some(CfaPattern::Gbrg);
        }

        None
    }

    /// Flip the pattern vertically (swap rows).
    ///
    /// What a `BOTTOM-UP` FITS needs when its height is **even**: `BAYERPAT` describes the top-down
    /// image, and reversing an even number of rows lands every row on the opposite phase. An odd
    /// height leaves the phases where they were, so the caller must not flip there — see
    /// `read_bayer_cfa`, which is where that parity is checked.
    pub fn flip_vertical(self) -> Self {
        match self {
            CfaPattern::Rggb => CfaPattern::Gbrg,
            CfaPattern::Gbrg => CfaPattern::Rggb,
            CfaPattern::Bggr => CfaPattern::Grbg,
            CfaPattern::Grbg => CfaPattern::Bggr,
        }
    }

    /// Flip the pattern horizontally (swap columns).
    /// Used when XBAYROFF is odd.
    pub fn flip_horizontal(self) -> Self {
        match self {
            CfaPattern::Rggb => CfaPattern::Grbg,
            CfaPattern::Grbg => CfaPattern::Rggb,
            CfaPattern::Bggr => CfaPattern::Gbrg,
            CfaPattern::Gbrg => CfaPattern::Bggr,
        }
    }

    /// Convert LibRaw's visible-origin pattern for consumers indexing the full raw buffer.
    pub(crate) fn at_raw_origin(self, top_margin: usize, left_margin: usize) -> Self {
        let mut pattern = self;
        if top_margin & 1 != 0 {
            pattern = pattern.flip_vertical();
        }
        if left_margin & 1 != 0 {
            pattern = pattern.flip_horizontal();
        }
        pattern
    }

    /// Get color index at position (y, x) in the Bayer pattern.
    /// Returns: 0=Red, 1=Green, 2=Blue
    #[inline(always)]
    pub fn color_at(&self, pos: Vec2us) -> usize {
        let row = pos.y & 1;
        let col = pos.x & 1;
        match self {
            CfaPattern::Rggb => [0, 1, 1, 2][(row << 1) | col],
            CfaPattern::Bggr => [2, 1, 1, 0][(row << 1) | col],
            CfaPattern::Grbg => [1, 0, 2, 1][(row << 1) | col],
            CfaPattern::Gbrg => [1, 2, 0, 1][(row << 1) | col],
        }
    }
}

/// Raw Bayer image data with metadata needed for demosaicing.
#[derive(Debug)]
pub(crate) struct BayerImage<'a> {
    /// Decoded or calibrated linear samples; calibration may put values outside `[0, 1]`.
    pub(crate) data: &'a [f32],
    /// Extent of the raw data buffer.
    pub(crate) raw: Size2us,
    /// Extent of the active/output image area.
    pub(crate) active: Size2us,
    /// Top-left corner of the active area within the raw buffer.
    pub(crate) margin: Vec2us,
    /// CFA pattern anchored at the full raw buffer origin.
    pub(crate) raw_cfa_pattern: CfaPattern,
}

impl<'a> BayerImage<'a> {
    /// Create a BayerImage with margins (libraw style).
    ///
    /// # Panics
    /// Panics under the conditions [`SensorLayout::validate`] names.
    pub(crate) fn with_margins(
        data: &'a [f32],
        layout: SensorLayout,
        raw_cfa_pattern: CfaPattern,
    ) -> Self {
        layout.validate(data.len());
        let SensorLayout {
            raw,
            active,
            margin,
        } = layout;

        debug_assert!(
            data.iter().all(|v| v.is_finite()),
            "BayerImage data contains NaN or Infinity values"
        );

        Self {
            data,
            raw,
            active,
            margin,
            raw_cfa_pattern,
        }
    }
}
