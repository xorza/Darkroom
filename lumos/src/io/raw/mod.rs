mod black_level;
pub(crate) mod demosaic;
pub(crate) mod error;
mod libraw;
pub(crate) mod raw_files;
mod sensor_layout;
mod unpacked_raw;

use libraw_sys as sys;
use std::ffi;
use std::fs;
use std::path::Path;

use crate::io::image::cfa::{CfaFrameInfo, CfaImage, CfaType};
use crate::io::image::error::ImageError;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::io::raw::error::RawError;
use crate::io::raw::libraw::Libraw;
use crate::io::raw::sensor_layout::SensorLayout;
use crate::io::raw::unpacked_raw::UnpackedRaw;

/// Camera-RAW extensions accepted by this decoder: the formats LibRaw decodes, as digiKam's
/// libkdcraw lists them, with Canon's CR3 and GoPro's GPR that later LibRaw added. `.hdr`, which
/// LibRaw reads for Hasselblad, is left out: it names Radiance HDR images far more often.
pub const RAW_EXTENSIONS: &[&str] = &[
    "3fr", "arw", "bay", "bmq", "cap", "cine", "cr2", "cr3", "crw", "cs1", "dc2", "dcr", "dng",
    "drf", "dsc", "erf", "fff", "gpr", "ia", "iiq", "k25", "kc2", "kdc", "mdc", "mef", "mos",
    "mrw", "nef", "nrw", "orf", "pef", "ptx", "pxn", "qtk", "raf", "raw", "rdc", "rw2", "rwl",
    "rwz", "sr2", "srf", "srw", "sti", "x3f",
];

/// The balance left to apply to a file's samples, normalized so its smallest multiplier is 1:
/// none for a monochrome sensor; unity where LibRaw reports the as-shot balance already in the
/// samples (`LIBRAW_ASWB_APPLIED` in `as_shot_wb_applied`: a Sony YCC pseudo-RAW, a Nikon sRAW, an
/// in-camera multi-exposure), as `cam_mul` would apply it twice; else `cam_mul`, a missing second
/// green and X-Trans's taken from the first, or none when a multiplier is not positive and finite.
fn camera_white_balance(
    cfa_type: Option<CfaType>,
    cam_mul: [f32; 4],
    as_shot_wb_applied: i32,
) -> Option<[f32; 4]> {
    if cfa_type == Some(CfaType::Mono) {
        return None;
    }
    if as_shot_wb_applied & 1 != 0 {
        return Some([1.0; 4]);
    }

    let mut multipliers = cam_mul;
    if matches!(cfa_type, Some(CfaType::XTrans(_))) || multipliers[3] == 0.0 {
        multipliers[3] = multipliers[1];
    }
    if multipliers
        .iter()
        .any(|multiplier| !multiplier.is_finite() || *multiplier <= 0.0)
    {
        return None;
    }

    let minimum = multipliers.iter().copied().fold(f32::MAX, f32::min);
    for multiplier in &mut multipliers {
        *multiplier /= minimum;
    }
    Some(multipliers)
}

/// LibRaw's `FC` macro: the colour index at (row, col) of a `filters` word, which holds two bits
/// for each of the 8 × 2 positions of its repeating block.
#[inline(always)]
pub(crate) const fn libraw_filter_color(filters: u32, row: usize, col: usize) -> usize {
    ((filters >> (((row << 1 & 0xE) | (col & 1)) << 1)) & 3) as usize
}

/// `c_char` is `i8` on some targets and `u8` on others; the byte value is what LibRaw stores.
fn xtrans_pattern_from_libraw(pattern: [[ffi::c_char; 6]; 6]) -> [[u8; 6]; 6] {
    pattern.map(|row| row.map(|color| u8::from_ne_bytes(color.to_ne_bytes())))
}

/// The mosaic LibRaw's state `data` describes — see [`CfaType::from_libraw`].
fn sensor_cfa_type(data: &sys::libraw_data_t) -> Result<Option<CfaType>, RawError> {
    let idata = &data.idata;
    Ok(CfaType::from_libraw(
        idata.filters,
        idata.colors,
        xtrans_pattern_from_libraw(idata.xtrans),
    )?)
}

/// Read `path` whole and open it in LibRaw, which then parses the bytes in place, polling the
/// cancel token of `context`. One input path on every system: LibRaw's memory datastream is its
/// simplest, and a path never meets a narrow or wide file API.
fn open(path: &Path, context: &LoadContext) -> Result<Libraw, ImageError> {
    context.check_cancelled(path)?;
    let file = fs::read(path).map_err(|source| ImageError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Libraw::open(file, &context.cancel).map_err(|source| ImageError::raw(path, source))
}

/// Open and unpack `path`.
fn open_raw(path: &Path, context: &LoadContext) -> Result<UnpackedRaw, ImageError> {
    let libraw = open(path, context)?;
    UnpackedRaw::unpack(libraw, path).map_err(|source| ImageError::raw(path, source))
}

/// Load a raw file as a linear image within `[0, 1]`: the light-frame preview.
///
/// A sensor lumos demosaics goes the science path — [`load_raw_cfa`]'s frame, its demosaic over
/// the visible area alone, then a clamp — so the preview and a calibrated frame cannot disagree
/// anywhere, the masked margins included. Anything else — a linear DNG, sRAW or Foveon, or an
/// exotic CFA — is LibRaw's own processing.
pub(crate) fn load_raw(path: &Path, context: &LoadContext) -> Result<LinearImage, ImageError> {
    let raw = open_raw(path, context)?;
    context.check_cancelled(path)?;
    raw.into_linear_image(context)
}

/// Read output dimensions and sensor layout without the expensive `libraw_unpack`: the file is
/// read whole, as every load reads it, but its sensor data is not decoded.
pub(crate) fn raw_cfa_frame_info(
    path: &Path,
    context: &LoadContext,
) -> Result<CfaFrameInfo, ImageError> {
    frame_info(&open(path, context)?).map_err(|source| ImageError::raw(path, source))
}

/// What the open `libraw` settles of its frame before `unpack`.
fn frame_info(libraw: &Libraw) -> Result<CfaFrameInfo, RawError> {
    let layout = SensorLayout::of(&libraw.data().sizes)?;
    let cfa_type = sensor_cfa_type(libraw.data())?.ok_or(RawError::NotACfaFrame)?;
    Ok(CfaFrameInfo {
        dimensions: ImageDimensions::new(layout.active, 1),
        cfa_type,
        // The file LibRaw parses in place, and the raw buffer it unpacks into, both held while
        // the frame is normalized out of them.
        decoder_bytes: libraw.file_len() + layout.raw.pixel_count() * size_of::<u16>(),
    })
}

/// Load a raw file as its CFA frame, un-demosaiced and unclamped: the calibration and science
/// path, where defects are repaired before any demosaic and a master's noise below its pedestal
/// stays in.
pub(crate) fn load_raw_cfa(path: &Path, context: &LoadContext) -> Result<CfaImage, ImageError> {
    let raw = open_raw(path, context)?;
    context.check_cancelled(path)?;
    raw.into_cfa_image()
        .map_err(|source| ImageError::raw(path, source))
}

#[cfg(all(test, feature = "real-data"))]
pub(crate) mod internals {
    use std::path::Path;

    use crate::io::image::error::ImageError;
    use crate::io::image::linear::LinearImage;
    use crate::io::image::load_context::LoadContext;
    use crate::io::raw::open_raw;

    /// Load a raw file through libraw's own demosaic at quality `user_qual`, the reference ours is
    /// compared with.
    pub(crate) fn load_raw_libraw_demosaic(
        path: &Path,
        user_qual: i32,
    ) -> Result<LinearImage, ImageError> {
        let mut raw = open_raw(path, &LoadContext::default())?;
        raw.libraw.params_mut().user_qual = user_qual;
        raw.processed_by_libraw()
            .map_err(|source| ImageError::raw(path, source))
    }
}

#[cfg(all(test, feature = "bench", feature = "real-data"))]
mod bench;
#[cfg(all(test, feature = "real-data"))]
mod quality_report;
#[cfg(test)]
mod tests;
