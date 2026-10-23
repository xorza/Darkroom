mod black_level;
pub(crate) mod error;
mod libraw;
pub(crate) mod libraw_filters;
mod raw_decoder;
pub(crate) mod raw_file;
pub(crate) mod raw_files;
mod sensor_layout;
mod unpacked_raw;

use std::ffi;
use std::io::Read;
use std::path::Path;

use common::FixedFile;
use libraw_sys as sys;

use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::channel_layout::ChannelLayout;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::linear_image::LinearImage;
use crate::io::decoder::Peek;
use crate::io::error::{ImageError, OverBudget};
use crate::io::frame_info::{DecoderBytes, FrameInfo};
use crate::io::load_ctx::LoadCtx;
use crate::io::raw::error::RawError;
use crate::io::raw::libraw::Libraw;
use crate::io::raw::libraw_filters::LibrawFilters;
use crate::io::raw::raw_decoder::RawDecoder;
use crate::io::raw::raw_file::RawFile;
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
    if matches!(cfa_type, Some(CfaType::Mosaic(Mosaic::XTrans(_)))) || multipliers[3] == 0.0 {
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

/// The sensor pattern LibRaw's state `data` describes — see [`LibrawFilters::cfa_type`].
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

/// Read `path` whole, for LibRaw to parse in place. One input path on every system: LibRaw's
/// memory datastream is its simplest, and a path never meets a narrow or wide file API.
/// A file longer than the context's memory limit is refused before it is read.
fn read(path: &Path, ctx: &LoadCtx) -> Result<RawFile, ImageError> {
    ctx.check_cancelled(path)?;
    let io_error = |source| ImageError::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut file = FixedFile::open(path).map_err(io_error)?;
    OverBudget::check(file.len(), ctx.memory_limit_bytes)
        .map_err(|budget| ImageError::raw(path, budget.into()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(io_error)?;
    Ok(RawFile { bytes })
}

/// `file`, read from `path`, opened in LibRaw, polling the cancel token of `ctx`.
fn open(file: RawFile, path: &Path, ctx: &LoadCtx) -> Result<Libraw, ImageError> {
    Libraw::open(file.bytes, &ctx.cancel).map_err(|source| ImageError::raw(path, source))
}

/// Open and unpack `file`, read from `path`, once the decode along the path `route` names is
/// admitted under the context's memory limit.
fn open_raw(
    file: RawFile,
    path: &Path,
    ctx: &LoadCtx,
    route: impl FnOnce(&Libraw) -> Result<RawPath, RawError>,
) -> Result<UnpackedRaw, ImageError> {
    let libraw = open(file, path, ctx)?;
    route(&libraw)
        .and_then(|route| admit(&libraw, route, ctx.memory_limit_bytes))
        .and_then(|()| UnpackedRaw::unpack(libraw, ctx.decode_threads))
        .map_err(|source| ImageError::raw(path, source))
}

/// What the decoder of the open `libraw` holds while it makes a mosaic: the file LibRaw parses in
/// place, until the sensor data is unpacked ([`UnpackedRaw::into_cfa_image`]), and the raw buffer
/// it unpacks into, beside the frame.
const fn decoder_bytes(libraw: &Libraw, layout: &SensorLayout) -> DecoderBytes {
    DecoderBytes {
        source: libraw.file_len(),
        beside: layout.raw.pixel_count() * size_of::<u16>(),
    }
}

/// Refuse a decode of the open `libraw` along `route` past `limit` bytes, from its header before
/// `unpack`: what it holds at its peak, its decoder and the frame `route` makes. LibRaw's own
/// processing reads the file again, so it holds the file beside the frame.
fn admit(libraw: &Libraw, route: RawPath, limit: u64) -> Result<(), RawError> {
    let layout = SensorLayout::of(&libraw.data().sizes)?;
    let decoder = decoder_bytes(libraw, &layout);
    let frame = route.frame_bytes(layout.active);
    let required = match route {
        RawPath::Mosaic => decoder.peak(frame),
        RawPath::Processed => decoder.source + decoder.beside + frame,
    };
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
pub(super) fn load_preview(path: &Path, ctx: &LoadCtx) -> Result<LinearImage, ImageError> {
    let raw = open_raw(read(path, ctx)?, path, ctx, |libraw| {
        preview_route(libraw, ctx)
    })?;
    ctx.check_cancelled(path)?;
    raw.into_linear_image(ctx)
        .map_err(|source| ImageError::raw(path, source))
}

/// The path a preview of the open `libraw` takes ([`RawPath::preview`]), once a mosaic's demosaic
/// is admitted under `ctx` beside its decode ([`FrameInfo::admit_demosaic`]).
fn preview_route(libraw: &Libraw, ctx: &LoadCtx) -> Result<RawPath, RawError> {
    let route = RawPath::preview(libraw)?;
    if route == RawPath::Mosaic {
        frame_info(libraw)?.admit_demosaic(ctx)?;
    }
    Ok(route)
}

/// What the CFA frame of `path` is, without the expensive `libraw_unpack`: the file is read whole,
/// as every load reads it, and kept for the load ([`load_cfa_read`]), but its sensor data is not
/// decoded.
pub(crate) fn cfa_peek(path: &Path, ctx: &LoadCtx) -> Result<Peek<RawFile>, ImageError> {
    let libraw = open(read(path, ctx)?, path, ctx)?;
    let info = frame_info(&libraw).map_err(|source| ImageError::raw(path, source))?;
    Ok(Peek {
        info,
        read: Some(RawFile {
            bytes: libraw.into_file(),
        }),
    })
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
        decoder: decoder_bytes(libraw, &layout),
        gain_bytes: 0,
    })
}

/// Load a raw file as its CFA frame, un-demosaiced and unclamped: the calibration and science
/// path, where defects are repaired before any demosaic and a master's noise below its pedestal
/// stays in.
pub(crate) fn load_cfa(path: &Path, ctx: &LoadCtx) -> Result<CfaImage, ImageError> {
    load_cfa_read(read(path, ctx)?, path, ctx)
}

/// [`load_cfa`] of `file`, read from `path` already: by the header read that settled the run's
/// shape ([`cfa_peek`]).
pub(crate) fn load_cfa_read(
    file: RawFile,
    path: &Path,
    ctx: &LoadCtx,
) -> Result<CfaImage, ImageError> {
    let raw = open_raw(file, path, ctx, |_| Ok(RawPath::Mosaic))?;
    ctx.check_cancelled(path)?;
    raw.into_cfa_image()
        .map_err(|source| ImageError::raw(path, source))
}

#[cfg(all(test, feature = "real-data"))]
pub(crate) mod internals {
    use std::path::Path;

    use crate::image::linear_image::LinearImage;
    use crate::io::error::ImageError;
    use crate::io::load_ctx::LoadCtx;
    use crate::io::raw::unpacked_raw::RawPath;
    use crate::io::raw::{open_raw, read};

    /// Load a raw file through libraw's own demosaic at quality `user_qual`, the reference ours is
    /// compared with.
    pub(crate) fn load_raw_libraw_demosaic(
        path: &Path,
        user_qual: i32,
    ) -> Result<LinearImage, ImageError> {
        let ctx = LoadCtx::default();
        let mut raw = open_raw(read(path, &ctx)?, path, &ctx, |_| Ok(RawPath::Processed))?;
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
