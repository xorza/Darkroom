//! X-Trans CFA demosaicing module.
//!
//! Provides demosaicing for Fujifilm X-Trans sensors which use a 6x6 CFA pattern
//! instead of the standard 2x2 Bayer pattern.
//!
//! The X-Trans pattern has ~55% green, ~22.5% red, and ~22.5% blue pixels arranged
//! so that every row and column contains all three colors.
//!
//! Uses the Markesteijn algorithm, one pass or three: directional interpolation with
//! homogeneity-based selection.

pub(crate) mod markesteijn;
pub(crate) mod xtrans_image;

#[cfg(test)]
pub(crate) mod internals {
    use crate::internals::cfa::XTRANS_PATTERN;
    use crate::io::image::cfa::xtrans_pattern::XTransPattern;
    use crate::io::raw::demosaic::xtrans::xtrans_image::XTransImage;
    use crate::math::size2us::Size2us;

    /// `data` as a frame of `size` under the test pattern.
    pub(crate) fn make_xtrans(data: &[f32], size: Size2us) -> XTransImage<'_> {
        XTransImage::new(data, size, XTRANS_PATTERN)
    }

    pub(crate) fn test_pattern_array() -> [[u8; 6]; 6] {
        *XTRANS_PATTERN.rows()
    }

    pub(crate) fn test_pattern() -> XTransPattern {
        XTRANS_PATTERN
    }
}

#[cfg(test)]
mod tests;
