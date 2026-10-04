mod black_level;
pub(crate) mod demosaic;
mod error;
pub(crate) mod raw_files;
mod sensor_layout;

use libraw_sys as sys;
use std::array;
use std::ffi;
#[cfg(unix)]
use std::ffi::CString;
use std::fs;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::slice;
use std::time::Instant;

use crate::io::cancelled::Cancelled;
use crate::io::image::error::ImageError;
use crate::io::raw::black_level::{BlackLevel, DngLevels, LibrawBlack};
use crate::io::raw::sensor_layout::SensorLayout;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

use rayon::prelude::*;

use crate::io::image::cfa::{CfaFrameInfo, CfaImage, CfaType, QUANTIZATION_SIGMA_PER_STEP};
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::image_provenance::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, RowOrder,
    SourceContainer, TransferProvenance,
};
use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags, SATURATION_FRACTION};
use crate::io::image::sample_domain::{Pedestal, SampleDomain, ScaleOrigin};
use imaginarium::Buffer2;

/// Camera-RAW extensions accepted by this decoder: the formats LibRaw decodes, as digiKam's
/// libkdcraw lists them, with Canon's CR3 and GoPro's GPR that later LibRaw added. `.hdr`, which
/// LibRaw reads for Hasselblad, is left out: it names Radiance HDR images far more often.
pub const RAW_EXTENSIONS: &[&str] = &[
    "3fr", "arw", "bay", "bmq", "cap", "cine", "cr2", "cr3", "crw", "cs1", "dc2", "dcr", "dng",
    "drf", "dsc", "erf", "fff", "gpr", "ia", "iiq", "k25", "kc2", "kdc", "mdc", "mef", "mos",
    "mrw", "nef", "nrw", "orf", "pef", "ptx", "pxn", "qtk", "raf", "raw", "rdc", "rw2", "rwl",
    "rwz", "sr2", "srf", "srw", "sti", "x3f",
];

/// An open libraw instance and, where the platform needed one, the file bytes it parses in place.
///
/// One owner for both, because they have one lifetime: `libraw_open_buffer` leaves libraw holding
/// a pointer into `buf`, so releasing the bytes first would leave it reading freed memory. The
/// handle is only ever reachable through [`Self::as_ptr`], which borrows `self`, so no caller can
/// hold it past the value that frees it.
#[derive(Debug)]
struct LibrawState {
    inner: *mut sys::libraw_data_t,
    buf: Option<Vec<u8>>,
}

impl LibrawState {
    /// Initialize libraw and open `path`, reading the file into memory where the platform's paths
    /// cannot go through libraw's narrow file API.
    fn open(path: &Path) -> Result<Self, ImageError> {
        // SAFETY: libraw_init returns a valid pointer or null on failure.
        let inner = unsafe { sys::libraw_init(0) };
        if inner.is_null() {
            return Err(raw_err(path, "libraw: Failed to initialize"));
        }

        // Owns `inner` from here, so a failure to open still frees it.
        let mut state = Self { inner, buf: None };
        state.buf = open_libraw_input(state.inner, path)?;
        Ok(state)
    }

    /// The libraw instance, borrowed for no longer than the state that frees it.
    const fn as_ptr(&self) -> *mut sys::libraw_data_t {
        self.inner
    }
}

impl Drop for LibrawState {
    fn drop(&mut self) {
        // Runs ahead of the fields, which is the order libraw needs: it may still be pointing into
        // `buf`, and `buf` is dropped only after this returns.
        // SAFETY: We own this pointer and it was allocated by libraw_init.
        unsafe { sys::libraw_close(self.inner) };
    }
}

/// RAII guard for `libraw_processed_image_t` to ensure proper cleanup.
#[derive(Debug)]
struct ProcessedImageGuard(*mut sys::libraw_processed_image_t);

impl Drop for ProcessedImageGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: We own this pointer and it was allocated by libraw_dcraw_make_mem_image.
            unsafe { sys::libraw_dcraw_clear_mem(self.0) };
        }
    }
}

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

pub(crate) fn raw_err(path: &Path, reason: impl Into<String>) -> ImageError {
    ImageError::Raw {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

/// Unpacked raw file data from libraw, ready for sensor-specific processing.
#[derive(Debug)]
struct UnpackedRaw {
    libraw: LibrawState,
    path: PathBuf,
    layout: SensorLayout,
    black_level: BlackLevel,
    visible_filters: u32,
    /// The mosaic lumos demosaics itself, or `None` when LibRaw has to process the image.
    cfa_type: Option<CfaType>,
    camera_white_balance: Option<[f32; 4]>,
    iso: Option<u32>,
    /// The shutter time in seconds, when LibRaw read one.
    exposure_time: Option<f64>,
    /// The sensor temperature in °C its makernotes state, for the cameras that record one.
    ccd_temp: Option<f64>,
    /// Whether LibRaw reads a raw zero as a dead photosite: its `zero_is_bad`, set for Panasonic
    /// and some cameras its size table identifies.
    zero_is_bad: bool,
    /// The raw value at which each LibRaw colour channel saturates, black included:
    /// [`SATURATION_FRACTION`] of the way from its black to its linear limit — `linear_max` where
    /// LibRaw knows it, else `maximum`.
    saturation: [f64; 4],
    /// Whether LibRaw's linearization curve is the identity up to `maximum`. A compressed format
    /// (Sony cRAW, Nikon lossy NEF, Canon C-RAW) maps its codes through a curve whose steps grow
    /// to several ADU in the highlights, so one ADU is no longer the quantization step.
    linear_curve: bool,
}

impl UnpackedRaw {
    /// Sensor ADU above black over `maximum − black`, from the file itself.
    fn sample_domain(&self) -> SampleDomain {
        SampleDomain {
            scale: self.black_level.span(),
            origin: ScaleOrigin::Declared,
            pedestal: Pedestal::Removed,
            // No RAW format states a unit for sensor counts, and inventing one here would make a
            // RAW frame disagree with a FITS frame that spells the same thing differently.
            unit: None,
        }
    }

    /// The flags the raw values themselves settle for the active area: [`QualityFlags::SATURATED`] at
    /// each channel's saturation level, and [`QualityFlags::NO_DATA`] on the zeros of a `zero_is_bad`
    /// camera; `None` when no photosite carries either.
    ///
    /// Saturation is decided here, on the raw value before black is subtracted, because after a
    /// dark subtraction and a flat division the level differs at every pixel. LibRaw's own
    /// processing replaces `zero_is_bad` zeros with a same-colour mean (`remove_zeroes`); lumos reads
    /// the raw buffer itself, so without the flag a dead photosite would normalize to
    /// `−black / span` and enter every stage as a measurement.
    fn decode_flags(&self) -> Result<Option<PixelFlags>, ImageError> {
        let raw = self.raw_image_slice()?;
        let SensorLayout {
            raw: raw_size,
            active,
            margin,
        } = self.layout;
        // Copied out: the closure runs on rayon workers, and `self` holds the libraw handle.
        let (zero_is_bad, saturation) = (self.zero_is_bad, self.saturation);
        let channel_at = self.channel_map();
        Ok(PixelFlags::from_fn(active, |index| {
            let (x, y) = (index % active.width, index / active.width);
            let value = raw[(y + margin.y) * raw_size.width + x + margin.x];
            if value == 0 && zero_is_bad {
                QualityFlags::NO_DATA
            } else if f64::from(value) >= saturation[channel_at(x, y)] {
                QualityFlags::SATURATED
            } else {
                QualityFlags::default()
            }
        }))
    }

    /// One ADU's uniform-error σ in the normalized domain, or `None` when a compressed curve makes
    /// the step vary.
    fn quantization_sigma(&self) -> Option<f32> {
        self.linear_curve
            .then(|| (f64::from(QUANTIZATION_SIGMA_PER_STEP) / self.black_level.span()) as f32)
    }

    /// Each visible pixel's LibRaw colour channel: the X-Trans pattern's colour, the Bayer
    /// `filters` word's (green on the second row as channel 3), or 0 for a monochrome sensor.
    /// Copied out of `self`, so the map can run on rayon workers while `self` holds libraw.
    fn channel_map(&self) -> impl Fn(usize, usize) -> usize + Sync + use<> {
        let (cfa_type, filters) = (self.cfa_type, self.visible_filters);
        move |x: usize, y: usize| match cfa_type {
            Some(CfaType::XTrans(pattern)) => usize::from(pattern.color_at(Vec2us::new(x, y))),
            Some(CfaType::Bayer(_)) => libraw_filter_color(filters, y, x),
            Some(CfaType::Mono) | None => 0,
        }
    }

    /// Get the raw u16 image pointer and total pixel count.
    /// Returns the pointer and count, or an error if null.
    fn raw_image_slice(&self) -> Result<&[u16], ImageError> {
        // SAFETY: the libraw instance is valid and unpack succeeded.
        let raw_image_ptr = unsafe { (*self.libraw.as_ptr()).rawdata.raw_image };
        if raw_image_ptr.is_null() {
            return Err(raw_err(&self.path, "libraw: raw_image is null"));
        }

        let pixel_count = self.layout.raw.pixel_count();

        // SAFETY: raw_image_ptr is valid (checked above), and dimensions were validated in
        // open_raw.
        Ok(unsafe { slice::from_raw_parts(raw_image_ptr, pixel_count) })
    }

    /// The visible area as a CFA frame: every sample `(v − black) / span`, unclamped — a dark's or
    /// a bias's noise below its pedestal is data the stack averages — with the flags the raw values
    /// settle. Consumes `self`, so LibRaw's state, tens of MB, is freed before a demosaic allocates
    /// its own.
    fn into_cfa_image(self) -> Result<CfaImage, ImageError> {
        let cfa_type = self.cfa_type.ok_or_else(|| not_a_cfa_frame(&self.path))?;
        let pixels =
            self.black_level
                .normalize(self.raw_image_slice()?, self.layout, self.channel_map());
        let metadata = ImageMetadata {
            domain: Some(self.sample_domain()),
            quantization_sigma: self.quantization_sigma(),
            saturation_flagged: true,
            iso: self.iso,
            exposure_time: self.exposure_time,
            ccd_temp: self.ccd_temp,
            header_dimensions: vec![self.layout.active.height, self.layout.active.width, 1],
            camera_white_balance: self.camera_white_balance,
            provenance: Some(ImageProvenance {
                container: SourceContainer::CameraRaw,
                decoder: DecoderProvenance::LibRaw,
                transfer: TransferProvenance::RawNormalized,
                color: ColorProvenance::SensorCfa,
                clipped: false,
                demosaic: DemosaicProvenance::None,
                // libraw hands back the visible area top-down; no RAW format stores it otherwise.
                row_order: RowOrder::TopDown,
            }),
            ..Default::default()
        };
        Ok(CfaImage {
            data: Buffer2::new(self.layout.active.width, self.layout.active.height, pixels),
            cfa_type,
            metadata,
            flags: self.decode_flags()?,
        })
    }

    /// [`load_raw`] past the open: the CFA path and a clamp, or LibRaw's own processing.
    fn into_linear_image(self, context: &LoadContext) -> Result<LinearImage, ImageError> {
        if self.cfa_type.is_some() {
            let path = self.path.clone();
            let mut image = self
                .into_cfa_image()?
                .demosaic(&context.cancel)
                .map_err(|Cancelled| ImageError::cancelled(&path))?;
            // The demosaic's own reach leaves the range, ~1.06 out of RCD and ~1.16 out of
            // Markesteijn at a step edge, and the frame arrives with its noise below black: the
            // preview clamps both, after the demosaic, so the interpolation sees the samples as
            // they are.
            for channel in 0..image.channels() {
                image
                    .channel_mut(channel)
                    .pixels_mut()
                    .par_iter_mut()
                    .for_each(|sample| *sample = sample.clamp(0.0, 1.0));
            }
            if let Some(provenance) = &mut image.metadata.provenance {
                provenance.clipped = true;
            }
            return Ok(image);
        }

        tracing::info!("no mosaic lumos demosaics; LibRaw processes the image");
        let domain = self.sample_domain();
        let demosaiced = self.demosaic_libraw_fallback()?;
        context.check_cancelled(&self.path)?;
        let dimensions = demosaiced.dimensions;
        let mut image = LinearImage::from_pixels(dimensions, demosaiced.pixels);
        image.metadata = ImageMetadata {
            domain: Some(domain),
            iso: self.iso,
            exposure_time: self.exposure_time,
            ccd_temp: self.ccd_temp,
            header_dimensions: vec![
                dimensions.height(),
                dimensions.width(),
                dimensions.channels(),
            ],
            camera_white_balance: self.camera_white_balance,
            provenance: Some(ImageProvenance {
                container: SourceContainer::CameraRaw,
                decoder: DecoderProvenance::LibRaw,
                transfer: TransferProvenance::RawNormalized,
                color: ColorProvenance::Unspecified,
                // LibRaw's 16-bit output divided by its integer maximum.
                clipped: true,
                demosaic: DemosaicProvenance::LibRaw,
                row_order: RowOrder::TopDown,
            }),
            ..Default::default()
        };
        Ok(image)
    }

    /// Process unknown CFA pattern using libraw's built-in demosaic.
    /// This is slower but handles exotic sensor patterns correctly.
    fn demosaic_libraw_fallback(&self) -> Result<LibrawDemosaiced, ImageError> {
        let demosaic_start = Instant::now();

        // Configure libraw for linear output (no gamma, no color conversion)
        // SAFETY: the libraw instance is valid
        unsafe {
            let inner = self.libraw.as_ptr();
            // Output in linear color space (no gamma curve)
            (*inner).params.gamm[0] = 1.0;
            (*inner).params.gamm[1] = 1.0;
            // No brightness adjustment
            (*inner).params.bright = 1.0;
            // LibRaw otherwise falls back to daylight WB when camera and auto WB are disabled.
            (*inner).params.user_mul = [1.0; 4];
            (*inner).params.use_auto_wb = 0;
            (*inner).params.use_camera_wb = 0;
            // Output 16-bit
            (*inner).params.output_bps = 16;
            // Linear color space (raw)
            (*inner).params.output_color = 0;
            // No auto-brightness
            (*inner).params.no_auto_bright = 1;
            // Scale by `maximum` as LibRaw read it, the span the domain records, not by a frame's
            // own brightest pixel when that lies within 25% of it.
            (*inner).params.adjust_maximum_thr = 0.0;
            // The sensor's rows and columns as they are: no turn by the EXIF orientation, which
            // would make a portrait frame H×W here alone, and no stretch by pixel aspect.
            (*inner).params.user_flip = 0;
            (*inner).params.use_fuji_rotate = 0;
        }

        // Run libraw's demosaic
        // SAFETY: the libraw instance is valid and configured
        let ret = unsafe { sys::libraw_dcraw_process(self.libraw.as_ptr()) };
        if ret != 0 {
            return Err(raw_err(
                &self.path,
                format!("libraw: dcraw_process failed, error code: {ret}"),
            ));
        }

        // Get the processed image
        // SAFETY: the libraw instance is valid and dcraw_process succeeded
        let mut errc: i32 = 0;
        let processed_ptr =
            unsafe { sys::libraw_dcraw_make_mem_image(self.libraw.as_ptr(), &raw mut errc) };
        if processed_ptr.is_null() || errc != 0 {
            return Err(raw_err(
                &self.path,
                format!("libraw: dcraw_make_mem_image failed, error code: {errc}"),
            ));
        }

        // Guard ensures cleanup even on early return or panic
        let _processed_guard = ProcessedImageGuard(processed_ptr);

        // Extract image data
        // SAFETY: processed_ptr is valid (checked above)
        let img_width = unsafe { (*processed_ptr).width } as usize;
        let img_height = unsafe { (*processed_ptr).height } as usize;
        let img_colors = unsafe { (*processed_ptr).colors } as usize;
        let img_bits = unsafe { (*processed_ptr).bits } as usize;
        let data_size = unsafe { (*processed_ptr).data_size } as usize;

        tracing::debug!(
            "libraw fallback: {}x{}x{}, {} bits, {} bytes",
            img_width,
            img_height,
            img_colors,
            img_bits,
            data_size
        );

        // libraw reports whatever the sensor gave it, and `output_color = 0` above skips the
        // conversion that would otherwise fold a four-colour sensor down to RGB. This is the trust
        // boundary: reject a geometry `ImageDimensions` cannot hold rather than assert on it.
        if img_width == 0 || img_height == 0 || (img_colors != 1 && img_colors != 3) {
            return Err(raw_err(
                &self.path,
                format!(
                    "libraw: demosaic produced unusable geometry {img_width}x{img_height}x{img_colors}"
                ),
            ));
        }

        // The data array is at the end of the struct (flexible array member)
        // SAFETY: processed_ptr is valid, data_size tells us the valid range
        let data_ptr = unsafe { (*processed_ptr).data.as_ptr() };

        // Use checked arithmetic to prevent overflow on extremely large images
        let pixel_count = img_width
            .checked_mul(img_height)
            .and_then(|v| v.checked_mul(img_colors))
            .expect("libraw: image dimensions overflow");

        // `output_bps = 16` above: anything else is not what was asked for.
        if img_bits != 16 {
            return Err(raw_err(
                &self.path,
                format!("libraw: demosaic produced {img_bits}-bit samples, not 16"),
            ));
        }
        let expected_size = pixel_count
            .checked_mul(2)
            .expect("libraw: expected_size overflow");
        if data_size < expected_size {
            return Err(raw_err(
                &self.path,
                format!("libraw: demosaic produced {data_size} bytes, not {expected_size}"),
            ));
        }
        // SAFETY: data_ptr points to valid u16 data of the calculated size
        let data_u16 = unsafe { slice::from_raw_parts(data_ptr.cast::<u16>(), pixel_count) };
        let pixels: Vec<f32> = data_u16.iter().map(|&v| f32::from(v) / 65535.0).collect();

        // Memory is freed automatically by _processed_guard when it goes out of scope

        let demosaic_elapsed = demosaic_start.elapsed();
        tracing::info!(
            "Libraw fallback demosaicing {}x{} took {:.2}ms",
            img_width,
            img_height,
            demosaic_elapsed.as_secs_f64() * 1000.0
        );

        Ok(LibrawDemosaiced {
            pixels,
            dimensions: ImageDimensions::new(Size2us::new(img_width, img_height), img_colors),
        })
    }
}

/// What libraw's own demosaic produced: interleaved samples normalized to `[0, 1]`, and the
/// geometry they are laid out by — libraw picks the output size and channel count itself, so
/// neither is known before the call, and both are checked there rather than trusted here.
#[derive(Debug)]
struct LibrawDemosaiced {
    pixels: Vec<f32>,
    dimensions: ImageDimensions,
}

/// Open and unpack a raw file using libraw.
///
/// Performs: libraw init, file open, unpack, dimension/color
/// validation, sensor type detection, and ISO extraction.
fn open_raw(path: &Path) -> Result<UnpackedRaw, ImageError> {
    unpack(LibrawState::open(path)?, path)
}

/// Unpack an opened file and read what lumos needs of it.
fn unpack(libraw: LibrawState, path: &Path) -> Result<UnpackedRaw, ImageError> {
    // Valid for the whole function: `libraw` owns it and outlives every use below.
    let inner = libraw.as_ptr();

    // SAFETY: inner is valid and open_buffer succeeded.
    let ret = unsafe { sys::libraw_unpack(inner) };
    if ret != 0 {
        return Err(raw_err(
            path,
            format!("libraw: Failed to unpack, error code: {ret}"),
        ));
    }

    // SAFETY: inner is valid and unpack succeeded, sizes struct is initialized.
    let raw_width = unsafe { (*inner).sizes.raw_width } as usize;
    let raw_height = unsafe { (*inner).sizes.raw_height } as usize;
    let width = unsafe { (*inner).sizes.width } as usize;
    let height = unsafe { (*inner).sizes.height } as usize;
    let top_margin = unsafe { (*inner).sizes.top_margin } as usize;
    let left_margin = unsafe { (*inner).sizes.left_margin } as usize;

    // Validate dimensions
    if raw_width == 0 || raw_height == 0 {
        return Err(raw_err(
            path,
            format!("libraw: Invalid raw dimensions: {raw_width}x{raw_height}"),
        ));
    }
    if width == 0 || height == 0 {
        return Err(raw_err(
            path,
            format!("libraw: Invalid output dimensions: {width}x{height}"),
        ));
    }
    if top_margin + height > raw_height || left_margin + width > raw_width {
        return Err(raw_err(
            path,
            format!(
                "libraw: Margins exceed raw dimensions: margins ({top_margin}, {left_margin}) + size ({width}, {height}) > raw ({raw_width}, {raw_height})"
            ),
        ));
    }
    // The raw buffer is read as rows of `raw_width` samples: any other pitch would shear it.
    // SAFETY: inner is valid and unpack succeeded, sizes struct is initialized.
    let raw_pitch = unsafe { (*inner).sizes.raw_pitch } as usize;
    if raw_pitch != 2 * raw_width {
        return Err(raw_err(
            path,
            format!("libraw: raw pitch {raw_pitch} bytes is not two per sample of {raw_width}"),
        ));
    }
    // A Fuji SuperCCD lays its photosites out 45° to the rows; neither demosaic reads that.
    // SAFETY: inner is valid and the file is open, so LibRaw has identified the sensor.
    let fuji_width = unsafe { sys::libraw_lumos_fuji_width(inner) };
    if fuji_width != 0 {
        return Err(raw_err(
            path,
            "libraw: a Fuji SuperCCD's 45° photosite layout is not supported",
        ));
    }

    // SAFETY: inner is valid, color struct is initialized after unpack.
    let black_raw = unsafe { (*inner).color.black };
    let maximum_raw = unsafe { (*inner).color.maximum };

    // Get sensor info from libraw metadata
    // SAFETY: inner is valid, idata struct is initialized after unpack.
    let visible_filters = unsafe { (*inner).idata.filters };
    let cfa_type = sensor_cfa_type(&libraw, path)?;

    tracing::debug!(
        "libraw: filters=0x{:08x}, cfa_type={:?}",
        visible_filters,
        cfa_type
    );

    // SAFETY: inner is valid; the color struct, the DNG levels in it included, and idata are
    // initialized after unpack.
    let black_level = unsafe {
        let color = &(*inner).color;
        BlackLevel::from_libraw(&LibrawBlack {
            black: black_raw,
            cblack: &color.cblack,
            maximum: maximum_raw,
            filters: visible_filters,
            dng: ((*inner).idata.dng_version != 0).then_some(DngLevels {
                black: color.dng_levels.dng_fblack,
                cblack: &color.dng_levels.dng_fcblack,
            }),
            masked: color.black_stat,
        })
    }
    .map_err(|source| raw_err(path, source.to_string()))?;

    // SAFETY: inner is valid, and the color struct is initialized after unpack.
    let camera_white_balance = unsafe {
        camera_white_balance(
            cfa_type,
            (*inner).color.cam_mul,
            (*inner).color.as_shot_wb_applied,
        )
    };
    let iso = extract_iso(inner);
    // SAFETY: inner is valid after unpack.
    let shutter = unsafe { (*inner).other.shutter };
    let exposure_time = (shutter > 0.0).then_some(f64::from(shutter));
    // SAFETY: inner is valid after unpack. LibRaw leaves -1000 where the makernotes state no sensor
    // temperature.
    let sensor_temperature = unsafe { (*inner).makernotes.common.SensorTemperature };
    let ccd_temp = (sensor_temperature > -273.15).then_some(f64::from(sensor_temperature));
    // SAFETY: inner is valid and the file is open, so LibRaw has identified the camera.
    let zero_is_bad = unsafe { sys::libraw_lumos_zero_is_bad(inner) } != 0;
    // SAFETY: inner is valid, and color.linear_max is initialized after unpack.
    let linear_max = unsafe { (*inner).color.linear_max };
    let saturation = array::from_fn(|channel| {
        let limit = f64::from(match linear_max[channel] {
            0 => maximum_raw,
            known => known,
        });
        let black = black_level.of_channel(channel);
        black + f64::from(SATURATION_FRACTION) * (limit - black)
    });
    // SAFETY: inner is valid, and color.curve is initialized after unpack.
    let curve = unsafe { &(*inner).color.curve };
    let last_code = (maximum_raw as usize).min(curve.len() - 1);
    let linear_curve = curve[..=last_code]
        .iter()
        .enumerate()
        .all(|(code, &value)| usize::from(value) == code);

    Ok(UnpackedRaw {
        libraw,
        path: path.to_path_buf(),
        layout: SensorLayout {
            raw: Size2us::new(raw_width, raw_height),
            active: Size2us::new(width, height),
            margin: Vec2us::new(left_margin, top_margin),
        },
        black_level,
        visible_filters,
        cfa_type,
        camera_white_balance,
        iso,
        exposure_time,
        ccd_temp,
        zero_is_bad,
        saturation,
        linear_curve,
    })
}

/// The mosaic an opened file's sensor delivers — see [`CfaType::from_libraw`].
fn sensor_cfa_type(libraw: &LibrawState, path: &Path) -> Result<Option<CfaType>, ImageError> {
    // SAFETY: the file is open, so LibRaw's image metadata, X-Trans pattern included, is set.
    let idata = unsafe { &(*libraw.as_ptr()).idata };
    CfaType::from_libraw(
        idata.filters,
        idata.colors,
        xtrans_pattern_from_libraw(idata.xtrans),
    )
    .map_err(|source| raw_err(path, source.to_string()))
}

/// What a refused libraw open means, for whichever call this host makes.
///
/// libraw answers one opaque code whether the path is unreadable or its
/// contents are not a raw, so the filesystem is asked which it was: an
/// unreadable path is an [`ImageError::Io`] on every host rather than a `Raw`
/// one on some. Cold — only a refused open pays for the syscall.
///
/// One home for both hosts, because the two are indistinguishable to a caller
/// and a wording that lives in two places drifts apart. The message names what
/// the caller lost, not the call that lost it: a buffer is how one host reaches
/// libraw, and the same rejected file must read alike on either.
fn open_refused(path: &Path, ret: i32) -> ImageError {
    if let Err(source) = fs::File::open(path) {
        return ImageError::Io {
            path: path.to_path_buf(),
            source,
        };
    }
    raw_err(
        path,
        format!("libraw: Failed to open file, error code: {ret}"),
    )
}

#[cfg(unix)]
fn open_libraw_input(
    inner: *mut sys::libraw_data_t,
    path: &Path,
) -> Result<Option<Vec<u8>>, ImageError> {
    let path_c = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| raw_err(path, "libraw: path contains an interior NUL byte"))?;
    // SAFETY: `inner` is valid and the C string remains alive for the complete call.
    let ret = unsafe { sys::libraw_open_file(inner, path_c.as_ptr()) };
    if ret != 0 {
        return Err(open_refused(path, ret));
    }
    Ok(None)
}

#[cfg(not(unix))]
fn open_libraw_input(
    inner: *mut sys::libraw_data_t,
    path: &Path,
) -> Result<Option<Vec<u8>>, ImageError> {
    let buf = fs::read(path).map_err(|e| ImageError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    // SAFETY: `inner` is valid and `buf` remains owned by the returned `UnpackedRaw`.
    let ret = unsafe { sys::libraw_open_buffer(inner, buf.as_ptr() as *const _, buf.len()) };
    if ret != 0 {
        return Err(open_refused(path, ret));
    }
    Ok(Some(buf))
}

/// Load a raw file as a linear image within `[0, 1]`: the light-frame preview.
///
/// A sensor lumos demosaics goes the science path — [`load_raw_cfa`]'s frame, its demosaic over
/// the visible area alone, then a clamp — so the preview and a calibrated frame cannot disagree
/// anywhere, the masked margins included. Anything else — a linear DNG, sRAW or Foveon, or an
/// exotic CFA — is LibRaw's own processing.
pub(crate) fn load_raw(path: &Path, context: &LoadContext) -> Result<LinearImage, ImageError> {
    context.check_cancelled(path)?;
    let raw = open_raw(path)?;
    context.check_cancelled(path)?;
    raw.into_linear_image(context)
}

/// Read output dimensions and sensor layout without the expensive `libraw_unpack`.
pub(crate) fn raw_cfa_frame_info(
    path: &Path,
    context: &LoadContext,
) -> Result<CfaFrameInfo, ImageError> {
    context.check_cancelled(path)?;
    // Frees libraw, and the file bytes it parses in place, on every return path.
    let libraw = LibrawState::open(path)?;
    context.check_cancelled(path)?;

    // SAFETY: opening succeeded, so the sizes struct is initialized.
    let width = unsafe { (*libraw.as_ptr()).sizes.width } as usize;
    let height = unsafe { (*libraw.as_ptr()).sizes.height } as usize;
    if width == 0 || height == 0 {
        return Err(raw_err(
            path,
            format!("libraw: Invalid output dimensions: {width}x{height}"),
        ));
    }
    let cfa_type = sensor_cfa_type(&libraw, path)?.ok_or_else(|| not_a_cfa_frame(path))?;
    Ok(CfaFrameInfo {
        dimensions: ImageDimensions::new((width, height), 1),
        cfa_type,
        // Only a `zero_is_bad` camera reports photosites with no measurement, and the identified
        // camera settles that before a pixel is read.
        // SAFETY: opening succeeded, so LibRaw has identified the camera.
        may_carry_nulls: unsafe { sys::libraw_lumos_zero_is_bad(libraw.as_ptr()) } != 0,
    })
}

/// Load a raw file as its CFA frame, un-demosaiced and unclamped: the calibration and science
/// path, where defects are repaired before any demosaic and a master's noise below its pedestal
/// stays in.
pub(crate) fn load_raw_cfa(path: &Path, context: &LoadContext) -> Result<CfaImage, ImageError> {
    context.check_cancelled(path)?;
    let raw = open_raw(path)?;
    context.check_cancelled(path)?;
    raw.into_cfa_image()
}

/// A CFA load of a sensor LibRaw delivers no mosaic for: a linear DNG, sRAW or Foveon, or an
/// exotic CFA. Such a file loads as a `LinearImage` through LibRaw's own processing.
fn not_a_cfa_frame(path: &Path) -> ImageError {
    raw_err(
        path,
        "not a CFA frame: LibRaw delivers this sensor's image already processed",
    )
}

/// Extract ISO from libraw metadata.
#[expect(
    clippy::cast_sign_loss,
    reason = "only a positive speed reaches the cast"
)]
fn extract_iso(inner: *mut sys::libraw_data_t) -> Option<u32> {
    // SAFETY: inner is valid after unpack.
    let iso_speed = unsafe { (*inner).other.iso_speed };
    if iso_speed > 0.0 {
        Some(iso_speed.round() as u32)
    } else {
        None
    }
}

#[cfg(all(test, feature = "real-data"))]
pub(crate) mod internals {
    use std::path::Path;

    use crate::io::image::error::ImageError;
    use crate::io::image::linear::LinearImage;
    use crate::io::raw::open_raw;

    /// Load a raw file through libraw's own demosaic, the reference ours is compared with.
    pub(crate) fn load_raw_libraw_demosaic(
        path: &Path,
        user_qual: i32,
    ) -> Result<LinearImage, ImageError> {
        let raw = open_raw(path)?;

        // Set demosaic quality before processing
        // SAFETY: `raw` owns the libraw instance for the rest of this function.
        unsafe {
            (*raw.libraw.as_ptr()).params.user_qual = user_qual;
        }

        let demosaiced = raw.demosaic_libraw_fallback()?;

        Ok(LinearImage::from_pixels(
            demosaiced.dimensions,
            demosaiced.pixels,
        ))
    }
}

#[cfg(all(test, feature = "bench", feature = "real-data"))]
mod bench;
#[cfg(all(test, feature = "real-data"))]
mod quality_report;
#[cfg(test)]
mod tests;
