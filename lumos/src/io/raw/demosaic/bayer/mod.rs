//! Bayer CFA demosaicing module.

mod rcd;
#[cfg(test)]
mod tests;

use common::CancelToken;

use crate::io::raw::demosaic::Cancelled;

/// Bayer CFA (Color Filter Array) pattern.
/// Represents the 2x2 pattern of color filters on the sensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    pub fn from_bayerpat(s: &str) -> Option<Self> {
        match s.trim().to_uppercase().as_str() {
            "RGGB" | "TRUE" => Some(CfaPattern::Rggb),
            "BGGR" => Some(CfaPattern::Bggr),
            "GRBG" => Some(CfaPattern::Grbg),
            "GBRG" => Some(CfaPattern::Gbrg),
            _ => None,
        }
    }

    /// Flip the pattern vertically (swap rows).
    /// Used when ROWORDER is BOTTOM-UP, since BAYERPAT assumes TOP-DOWN.
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

    /// Get color index at position (y, x) in the Bayer pattern.
    /// Returns: 0=Red, 1=Green, 2=Blue
    #[inline(always)]
    pub fn color_at(&self, y: usize, x: usize) -> usize {
        let row = y & 1;
        let col = x & 1;
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
    /// Raw Bayer pixel data (normalized to 0.0-1.0)
    pub(crate) data: &'a [f32],
    /// Width of the raw data buffer
    pub(crate) raw_width: usize,
    /// Height of the raw data buffer
    pub(crate) raw_height: usize,
    /// Width of the active/output image area
    pub(crate) width: usize,
    /// Height of the active/output image area
    pub(crate) height: usize,
    /// Top margin (offset from raw to active area)
    pub(crate) top_margin: usize,
    /// Left margin (offset from raw to active area)
    pub(crate) left_margin: usize,
    /// CFA pattern
    pub(crate) cfa: CfaPattern,
}

impl<'a> BayerImage<'a> {
    /// Create a BayerImage with margins (libraw style).
    ///
    /// # Panics
    /// Panics if:
    /// - `data.len() != raw_width * raw_height`
    /// - `top_margin + height > raw_height`
    /// - `left_margin + width > raw_width`
    /// - `width == 0` or `height == 0`
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn with_margins(
        data: &'a [f32],
        raw_width: usize,
        raw_height: usize,
        width: usize,
        height: usize,
        top_margin: usize,
        left_margin: usize,
        cfa: CfaPattern,
    ) -> Self {
        assert!(
            width > 0 && height > 0,
            "Output dimensions must be non-zero: {}x{}",
            width,
            height
        );
        assert!(
            raw_width > 0 && raw_height > 0,
            "Raw dimensions must be non-zero: {}x{}",
            raw_width,
            raw_height
        );
        assert!(
            data.len() == raw_width * raw_height,
            "Data length {} doesn't match raw dimensions {}x{}={}",
            data.len(),
            raw_width,
            raw_height,
            raw_width * raw_height
        );
        assert!(
            top_margin + height <= raw_height,
            "Top margin {} + height {} exceeds raw height {}",
            top_margin,
            height,
            raw_height
        );
        assert!(
            left_margin + width <= raw_width,
            "Left margin {} + width {} exceeds raw width {}",
            left_margin,
            width,
            raw_width
        );

        debug_assert!(
            data.iter().all(|v| v.is_finite()),
            "BayerImage data contains NaN or Infinity values"
        );

        Self {
            data,
            raw_width,
            raw_height,
            width,
            height,
            top_margin,
            left_margin,
            cfa,
        }
    }
}

/// Demosaic a Bayer CFA image to RGB using the RCD algorithm.
///
/// Returns planar channels `[R, G, B]`, each `width * height`, cropped to the
/// active area.
pub(crate) fn demosaic_bayer(
    bayer: &BayerImage,
    cancel: &CancelToken,
) -> Result<[Vec<f32>; 3], Cancelled> {
    rcd::rcd_demosaic(bayer, cancel)
}
