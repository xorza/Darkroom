mod black_level;
pub(crate) mod error;
mod libraw;
pub(crate) mod libraw_filters;
mod raw_decoder;
pub(crate) mod raw_files;
mod sensor_layout;
mod unpacked_raw;

use std::ffi;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use libraw_sys as sys;

use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::channel_layout::ChannelLayout;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::linear_image::LinearImage;
use crate::io::error::{ImageError, OverBudget};
use crate::io::frame_info::FrameInfo;
use crate::io::load_context::LoadContext;
use crate::io::raw::error::RawError;
use crate::io::raw::libraw::Libraw;
use crate::io::raw::libraw_filters::LibrawFilters;
use crate::io::raw::raw_decoder::RawDecoder;
use crate::io::raw::sensor_layout::SensorLayout;
use crate::io::raw::unpacked_raw::{RawPath, UnpackedRaw};

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

/// LibRaw's `FC` macro: the color index at (row, col) of a `filters` word, which holds two bits
/// for each of the 8 × 2 positions of its repeating block.
#[inline(always)]
pub(crate) const fn filter_color(filters: u32, row: usize, col: usize) -> usize {
    ((filters >> ((((row << 1) & 0xE) | (col & 1)) << 1)) & 3) as usize
}

/// `c_char` is `i8` on some targets and `u8` on others; the byte value is what LibRaw stores.
fn xtrans_pattern_from_libraw(pattern: [[ffi::c_char; 6]; 6]) -> [[u8; 6]; 6] {
    pattern.map(|row| row.map(|color| u8::from_ne_bytes(color.to_ne_bytes())))
}

/// The colors LibRaw's `cdesc` names, one letter each, its terminator left off.
fn color_description(cdesc: [ffi::c_char; 5]) -> [u8; 4] {
    let [a, b, c, d, _] = cdesc.map(|letter| u8::from_ne_bytes(letter.to_ne_bytes()));
    [a, b, c, d]
}

/// The mosaic LibRaw's state `data` describes — see [`LibrawFilters::cfa_type`].
fn sensor_cfa_type(data: &sys::libraw_data_t) -> Result<Option<CfaType>, RawError> {
    let idata = &data.idata;
    Ok(LibrawFilters {
        filters: idata.filters,
        colors: idata.colors,
        cdesc: color_description(idata.cdesc),
        xtrans: xtrans_pattern_from_libraw(idata.xtrans),
    }
    .cfa_type()?)
}

/// Read `path` whole and open it in LibRaw, which then parses the bytes in place, polling the
/// cancel token of `context`. One input path on every system: LibRaw's memory datastream is its
/// simplest, and a path never meets a narrow or wide file API.
/// A file longer than the context's memory limit is refused before it is read.
fn open(path: &Path, context: &LoadContext) -> Result<Libraw, ImageError> {
    context.check_cancelled(path)?;
    let io_error = |source| ImageError::Io {
        path: path.to_path_buf(),
        source,
    };
    let file = File::open(path).map_err(io_error)?;
    let len = file.metadata().map_err(io_error)?.len();
    OverBudget::check(len, context.memory_limit_bytes)
        .map_err(|budget| ImageError::raw(path, budget.into()))?;
    let mut bytes = Vec::with_capacity(usize::try_from(len).expect("a file within the limit"));
    // The length admitted, not what the file grew to since.
    file.take(len).read_to_end(&mut bytes).map_err(io_error)?;
    Libraw::open(bytes, &context.cancel).map_err(|source| ImageError::raw(path, source))
}

/// Open and unpack `path`, once the decode along the path `route` names is admitted under the
/// context's memory limit.
fn open_raw(
    path: &Path,
    context: &LoadContext,
    route: impl FnOnce(&Libraw) -> Result<RawPath, RawError>,
) -> Result<UnpackedRaw, ImageError> {
    let libraw = open(path, context)?;
    route(&libraw)
        .and_then(|route| admit(&libraw, route, context.memory_limit_bytes))
        .and_then(|()| UnpackedRaw::unpack(libraw, context.decode_threads))
        .map_err(|source| ImageError::raw(path, source))
}

/// What the decoder of the open `libraw` holds beside the frame it makes: the file LibRaw parses in
/// place, and the raw buffer it unpacks into.
const fn decoder_bytes(libraw: &Libraw, layout: &SensorLayout) -> usize {
    libraw.file_len() + layout.raw.pixel_count() * size_of::<u16>()
}

/// Refuse a decode of the open `libraw` along `route` past `limit` bytes, from its header before
/// `unpack`: what it holds at its peak, its decoder and the frame `route` makes.
fn admit(libraw: &Libraw, route: RawPath, limit: u64) -> Result<(), RawError> {
    let layout = SensorLayout::of(&libraw.data().sizes)?;
    let required =
        decoder_bytes(libraw, &layout) + layout.active.pixel_count() * route.pixel_bytes();
    OverBudget::check(
        u64::try_from(required).expect("a byte count fits u64"),
        limit,
    )?;
    Ok(())
}

/// Load a raw file as a linear image within `[0, 1]`: the light-frame preview.
///
/// A sensor lumos demosaics goes the science path — [`load_cfa`]'s frame, its demosaic over
/// the visible area alone, then a clamp — so the preview and a calibrated frame cannot disagree
/// anywhere, the masked margins included. Anything else — a linear DNG or sRAW, or an exotic
/// three-color CFA — is LibRaw's own processing.
pub(crate) fn load_preview(path: &Path, context: &LoadContext) -> Result<LinearImage, ImageError> {
    let raw = open_raw(path, context, RawPath::preview)?;
    context.check_cancelled(path)?;
    raw.into_linear_image(context)
        .map_err(|source| ImageError::raw(path, source))
}

/// Read output dimensions and sensor layout without the expensive `libraw_unpack`: the file is
/// read whole, as every load reads it, but its sensor data is not decoded.
pub(crate) fn cfa_frame_info(path: &Path, context: &LoadContext) -> Result<FrameInfo, ImageError> {
    frame_info(&open(path, context)?).map_err(|source| ImageError::raw(path, source))
}

/// The refusals LibRaw's identify stage settles, before any decode, which the header read and the
/// load share: a Fuji SuperCCD's 45° layout, which neither demosaic reads; a Phase One IIQ, whose
/// black LibRaw subtracts only in its own processing; a float DNG, which LibRaw quantizes on
/// unpack. Returns the decoder the rest of the decode reads.
fn identified(libraw: &Libraw) -> Result<RawDecoder, RawError> {
    if libraw.super_ccd() {
        return Err(RawError::SuperCcd);
    }
    match libraw.decoder() {
        RawDecoder::PhaseOne => Err(RawError::PhaseOne),
        RawDecoder::FloatingPoint => Err(RawError::FloatingPoint),
        decoder => Ok(decoder),
    }
}

/// What the open `libraw` settles of its frame before `unpack`.
///
/// # Errors
///
/// [`identified`]'s refusals, and [`RawError::MosaicSetAtDecode`] for a decoder that settles the
/// mosaic only as it decodes: the header's pattern is not the frame's.
fn frame_info(libraw: &Libraw) -> Result<FrameInfo, RawError> {
    if identified(libraw)? == RawDecoder::FiltersAtDecode {
        return Err(RawError::MosaicSetAtDecode);
    }
    let layout = SensorLayout::of(&libraw.data().sizes)?;
    let cfa_type = sensor_cfa_type(libraw.data())?.ok_or(RawError::NotACfaFrame)?;
    Ok(FrameInfo {
        dimensions: ImageDimensions::new(layout.active, ChannelLayout::Mono),
        cfa_type,
        decoder_bytes: decoder_bytes(libraw, &layout),
    })
}

/// Load a raw file as its CFA frame, un-demosaiced and unclamped: the calibration and science
/// path, where defects are repaired before any demosaic and a master's noise below its pedestal
/// stays in.
pub(crate) fn load_cfa(path: &Path, context: &LoadContext) -> Result<CfaImage, ImageError> {
    let raw = open_raw(path, context, |_| Ok(RawPath::Mosaic))?;
    context.check_cancelled(path)?;
    raw.into_cfa_image()
        .map_err(|source| ImageError::raw(path, source))
}

#[cfg(all(test, feature = "real-data"))]
pub(crate) mod internals {
    use std::path::Path;

    use crate::image::linear_image::LinearImage;
    use crate::io::error::ImageError;
    use crate::io::load_context::LoadContext;
    use crate::io::raw::open_raw;
    use crate::io::raw::unpacked_raw::RawPath;

    /// Load a raw file through libraw's own demosaic at quality `user_qual`, the reference ours is
    /// compared with.
    pub(crate) fn load_raw_libraw_demosaic(
        path: &Path,
        user_qual: i32,
    ) -> Result<LinearImage, ImageError> {
        let mut raw = open_raw(path, &LoadContext::default(), |_| Ok(RawPath::Processed))?;
        raw.libraw.params_mut().user_qual = user_qual;
        raw.processed_by_libraw()
            .map_err(|source| ImageError::raw(path, source))
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(all(test, feature = "real-data"))]
mod quality_report;
#[cfg(test)]
mod tests;
