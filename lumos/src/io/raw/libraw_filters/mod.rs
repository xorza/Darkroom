//! The mosaic LibRaw's report of a sensor's color filters means.

use libraw::ImageData;

use crate::image::cfa::bayer_pattern::BayerPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::cfa::xtrans_pattern::XTransPattern;
use crate::image::error::XTransPatternError;
use crate::io::raw;

/// The mosaic a camera-RAW sensor delivers, from LibRaw's `filters`, `colors` and `cdesc` and
/// its visible-origin X-Trans pattern, or `None` when LibRaw has to produce the image itself.
///
/// `None` covers `filters == 0` with three colors — a linear DNG or sRAW, whose samples LibRaw
/// unpacks already per pixel and so never as a mosaic — a filter word that is neither X-Trans
/// nor a 2×2 Bayer phase, and any sensor whose colors are not LibRaw's red, green and blue: a
/// word reads color 3 as a second green only where `cdesc` says so, and Sony's RGBE or Nikon's
/// CMYG laid out in a Bayer word's phases would otherwise demosaic as RGB with the wrong
/// filters. An X-Trans sensor whose layout is not one is an error: the file is corrupt, not
/// exotic.
pub(crate) fn cfa_type(idata: &ImageData) -> Result<Option<CfaType>, XTransPatternError> {
    let ImageData {
        filters,
        colors,
        cdesc,
        xtrans,
        ..
    } = *idata;
    if colors == 1 {
        return Ok(Some(CfaType::Mono));
    }
    if colors != 3 || &cdesc != b"RGBG" {
        return Ok(None);
    }
    Ok(match filters {
        0 => None,
        // LibRaw's marker for the 6×6 X-Trans layout, whose pattern it keeps apart.
        libraw::XTRANS_FILTERS => {
            Some(CfaType::Mosaic(Mosaic::XTrans(XTransPattern::new(xtrans)?)))
        }
        _ => bayer(filters).map(|pattern| CfaType::Mosaic(Mosaic::Bayer(pattern))),
    })
}

/// The Bayer phase LibRaw's `filters` word holds, which holds the color of each position of an
/// 8 × 2 block (see [`raw::filter_color`]). Color indices: 0=Red, 1=Green, 2=Blue, 3=Green2.
///
/// Returns `None` when the word is not a 2×2 Bayer phase repeated down all eight rows —
/// X-Trans, monochrome, and other exotic sensors all land here.
fn bayer(filters: u32) -> Option<BayerPattern> {
    // Each byte holds two rows; a 2-row period repeats the first byte through the word.
    if filters != (filters & 0xff) * 0x0101_0101 {
        return None;
    }
    let color_at = |row: usize, col: usize| raw::filter_color(filters, row, col);

    let c00 = color_at(0, 0);
    let c01 = color_at(0, 1);
    let c10 = color_at(1, 0);
    let c11 = color_at(1, 1);

    let is_red = |c: usize| c == 0;
    // Both green indices count as green.
    let is_green = |c: usize| c == 1 || c == 3;
    let is_blue = |c: usize| c == 2;

    if is_red(c00) && is_green(c01) && is_green(c10) && is_blue(c11) {
        return Some(BayerPattern::Rggb);
    }
    if is_blue(c00) && is_green(c01) && is_green(c10) && is_red(c11) {
        return Some(BayerPattern::Bggr);
    }
    if is_green(c00) && is_red(c01) && is_blue(c10) && is_green(c11) {
        return Some(BayerPattern::Grbg);
    }
    if is_green(c00) && is_blue(c01) && is_red(c10) && is_green(c11) {
        return Some(BayerPattern::Gbrg);
    }

    None
}

#[cfg(test)]
mod tests;
