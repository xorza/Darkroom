pub(crate) mod demosaic;
mod error;
mod normalize;
pub(crate) mod raw_files;

use libraw_sys as sys;
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
use crate::io::raw::error::BlackLevelError;
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
use crate::io::image::sample_domain::{Pedestal, SampleDomain, ScaleOrigin};
use crate::io::raw::demosaic::sensor_layout::SensorLayout;
use crate::io::raw::demosaic::xtrans::XTransNormalization;
use crate::io::raw::demosaic::xtrans::xtrans_pattern::XTransPattern;
use common::CancelToken;
use demosaic::bayer::{BayerImage, CfaPattern, rcd};
use demosaic::xtrans;
use imaginarium::Buffer2;

use normalize::{normalize_u16_to_f32_into, normalize_u16_to_f32_parallel};

/// Camera-RAW extensions accepted by this decoder.
pub const RAW_EXTENSIONS: &[&str] = &["raf", "cr2", "cr3", "nef", "arw", "dng"];

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

#[derive(Debug)]
pub(crate) struct BlackRepeat {
    /// Extent of the repeating tile the deltas cover.
    size: Size2us,
    delta_norm: Box<[f32]>,
}

impl BlackRepeat {
    #[inline(always)]
    fn at_visible(&self, row: usize, col: usize) -> f32 {
        self.delta_norm[(row % self.size.height) * self.size.width + col % self.size.width]
    }

    #[inline(always)]
    fn at_raw(&self, raw_row: usize, raw_col: usize, margin: Vec2us) -> f32 {
        // LibRaw defines repeat phase after cropping, so margins shift full-buffer coordinates.
        let row = (raw_row % self.size.height + self.size.height - margin.y % self.size.height)
            % self.size.height;
        let col = (raw_col % self.size.width + self.size.width - margin.x % self.size.width)
            % self.size.width;
        self.delta_norm[row * self.size.width + col]
    }
}

#[derive(Debug)]
struct BlackLevel {
    per_channel: [f32; 4],
    common: f32,
    /// `maximum − black` in ADU: what samples are divided by. An integer, so exact in f32.
    span: f32,
    channel_delta_norm: [f32; 4],
    repeat: Option<BlackRepeat>,
}

impl BlackLevel {
    /// The span the samples are divided by: `maximum − black`, in ADU.
    ///
    /// What one normalized unit is worth for this file. It differs between frames — libraw reads
    /// `maximum` per camera and per ISO, and `black` per frame — so two frames convert by the ratio
    /// of their spans (`SampleDomain::conversion_to`).
    const fn span(&self) -> f32 {
        self.span
    }
}

/// Replicate libraw's `adjust_bl()` black-level consolidation.
///
/// See libraw `utils_libraw.cpp:464-540` for the reference C++ implementation.
fn consolidate_black_levels(
    cblack_raw: &[u32; 4104],
    black_raw: u32,
    maximum_raw: u32,
    visible_filters: u32,
) -> Result<BlackLevel, BlackLevelError> {
    // Folded in place, as libraw folds its own: 16 KiB, too large a copy for the stack.
    let mut cblack = cblack_raw.to_vec();
    let mut black = black_raw;

    if cblack[4] > 0 && cblack[5] > 0 {
        let pattern_size = (cblack[4] as usize).checked_mul(cblack[5] as usize).ok_or(
            BlackLevelError::SpatialPatternOverflow {
                width: cblack[5],
                height: cblack[4],
            },
        )?;
        let capacity = cblack.len() - 6;
        if pattern_size > capacity {
            return Err(BlackLevelError::SpatialPatternTooLarge {
                width: cblack[5],
                height: cblack[4],
                capacity,
            });
        }
    }

    // Step 1: Fold spatial pattern into per-channel values.
    // For Bayer sensors with ~2x2 spatial pattern:
    if visible_filters > 1000 && cblack[4].div_ceil(2) == 1 && cblack[5].div_ceil(2) == 1 {
        let mut clrs = [0u32; 4];
        let mut last_g: Option<usize> = None;
        let mut g_count = 0;
        for (c, clr) in clrs.iter_mut().enumerate() {
            let row = c / 2;
            let col = c % 2;
            *clr = libraw_filter_color(visible_filters, row, col) as u32;
            if *clr == 1 {
                g_count += 1;
                last_g = Some(c);
            }
        }
        // If two greens found, remap second green to channel 3 (G2)
        if g_count > 1
            && let Some(lg) = last_g
        {
            clrs[lg] = 3;
        }
        for c in 0..4 {
            let pattern_idx =
                6 + (c / 2) % cblack[4] as usize * cblack[5] as usize + c % 2 % cblack[5] as usize;
            cblack[clrs[c] as usize] += cblack[pattern_idx];
        }
        cblack[4] = 0;
        cblack[5] = 0;
    } else if visible_filters <= 1000 && cblack[4] == 1 && cblack[5] == 1 {
        // X-Trans / Fuji RAF DNG: 1x1 spatial pattern
        for c in 0..4 {
            cblack[c] += cblack[6];
        }
        cblack[4] = 0;
        cblack[5] = 0;
    }

    // Step 2: Extract common minimum from per-channel values.
    let common_ch = cblack[..4].iter().copied().min().unwrap();
    for val in &mut cblack[..4] {
        *val -= common_ch;
    }
    black += common_ch;

    // Step 3: Handle remaining spatial pattern (rare).
    if cblack[4] > 0 && cblack[5] > 0 {
        let pattern_size = (cblack[4] * cblack[5]) as usize;
        let mut common_spatial = cblack[6];
        for c in 1..pattern_size {
            if cblack[6 + c] < common_spatial {
                common_spatial = cblack[6 + c];
            }
        }
        let mut nonzero = 0;
        for c in 0..pattern_size {
            cblack[6 + c] -= common_spatial;
            if cblack[6 + c] != 0 {
                nonzero += 1;
            }
        }
        black += common_spatial;
        if nonzero == 0 {
            cblack[4] = 0;
            cblack[5] = 0;
        }
    }

    let mut per_channel = [0f32; 4];
    for c in 0..4 {
        per_channel[c] = (cblack[c] + black) as f32;
    }
    let common = black as f32;
    let effective_max = maximum_raw as f32 - common;
    // File-derived metadata: a corrupt RAW can report maximum <= black. Return an error rather
    // than panicking at this trust boundary.
    if effective_max <= 0.0 {
        return Err(BlackLevelError::BlackExceedsMaximum {
            black,
            maximum: maximum_raw,
        });
    }
    let span = effective_max;
    let mut channel_delta_norm = [0f32; 4];
    for c in 0..4 {
        channel_delta_norm[c] = (per_channel[c] - common) / span;
    }
    let repeat = if cblack[4] > 0 && cblack[5] > 0 {
        let height = cblack[4] as usize;
        let width = cblack[5] as usize;
        let size = Size2us::new(width, height);
        Some(BlackRepeat {
            size,
            delta_norm: cblack[6..6 + size.pixel_count()]
                .iter()
                .map(|&delta| delta as f32 / span)
                .collect(),
        })
    } else {
        None
    };

    tracing::debug!(
        "Black levels: common={common}, per_channel={per_channel:?}, \
         delta_norm={channel_delta_norm:?}, repeat={}x{}, span={span}",
        repeat.as_ref().map_or(0, |pattern| pattern.size.width),
        repeat.as_ref().map_or(0, |pattern| pattern.size.height)
    );

    Ok(BlackLevel {
        per_channel,
        common,
        span,
        channel_delta_norm,
        repeat,
    })
}

fn canonical_camera_white_balance(
    cfa_type: Option<CfaType>,
    cam_mul: [f32; 4],
) -> Option<[f32; 4]> {
    if cfa_type == Some(CfaType::Mono) {
        return None;
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

/// Apply residual black correction to Bayer data.
///
/// Operates on data already normalized with the common black level.
/// LibRaw's `filters` is visible-origin, while `data` is the full raw buffer.
/// Results are clamped to the direct light-frame `[0, 1]` contract.
fn apply_bayer_black_corrections(
    data: &mut [f32],
    raw_width: usize,
    margin: Vec2us,
    visible_filters: u32,
    delta_norm: &[f32; 4],
    repeat: Option<&BlackRepeat>,
) {
    let has_correction =
        repeat.is_some() || delta_norm.iter().any(|&delta| delta.abs() > f32::EPSILON);
    if !has_correction {
        return;
    }

    data.par_chunks_mut(raw_width)
        .enumerate()
        .for_each(|(row, row_data)| {
            for (col, pixel) in row_data.iter_mut().enumerate() {
                let ch = raw_filter_color(visible_filters, row, col, margin);
                let repeat_delta = repeat.map_or(0.0, |pattern| pattern.at_raw(row, col, margin));
                *pixel = (*pixel - delta_norm[ch] - repeat_delta).clamp(0.0, 1.0);
            }
        });
}

/// LibRaw's `FC` macro: the colour index at (row, col) of a `filters` word, which holds two bits
/// for each of the 8 × 2 positions of its repeating block.
#[inline(always)]
pub(crate) const fn libraw_filter_color(filters: u32, row: usize, col: usize) -> usize {
    ((filters >> (((row << 1 & 0xE) | (col & 1)) << 1)) & 3) as usize
}

#[inline(always)]
const fn raw_filter_color(
    visible_filters: u32,
    raw_row: usize,
    raw_col: usize,
    margin: Vec2us,
) -> usize {
    libraw_filter_color(
        visible_filters,
        raw_row.wrapping_sub(margin.y),
        raw_col.wrapping_sub(margin.x),
    )
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

#[derive(Debug, Clone, Copy)]
enum ChannelBlackDelta {
    LibRawFilter {
        visible_filters: u32,
        values: [f32; 4],
    },
    XTrans {
        visible_pattern: XTransPattern,
        values: [f32; 3],
    },
}

impl ChannelBlackDelta {
    #[inline(always)]
    const fn at_visible(&self, row: usize, col: usize) -> f32 {
        match self {
            ChannelBlackDelta::LibRawFilter {
                visible_filters,
                values,
            } => values[libraw_filter_color(*visible_filters, row, col)],
            ChannelBlackDelta::XTrans {
                visible_pattern,
                values,
            } => values[visible_pattern.color_at(Vec2us::new(col, row)) as usize],
        }
    }
}

fn normalize_active_area<const CLAMP: bool>(
    raw_data: &[u16],
    layout: SensorLayout,
    black: f32,
    span: f32,
    channel_delta: Option<ChannelBlackDelta>,
    repeat: Option<&BlackRepeat>,
) -> Vec<f32> {
    let output_size = layout.active.pixel_count();
    let mut pixels = vec![0.0f32; output_size];
    pixels
        .par_chunks_mut(layout.active.width)
        .enumerate()
        .for_each(|(y, row)| {
            let raw_y = layout.margin.y + y;
            let src_start = raw_y * layout.raw.width + layout.margin.x;
            let source = &raw_data[src_start..src_start + layout.active.width];
            normalize_u16_to_f32_into::<CLAMP>(source, row, black, span);

            if channel_delta.is_some() || repeat.is_some() {
                for (x, pixel) in row.iter_mut().enumerate() {
                    let channel_correction = channel_delta
                        .as_ref()
                        .map_or(0.0, |delta| delta.at_visible(y, x));
                    let repeat_delta = repeat.map_or(0.0, |pattern| pattern.at_visible(y, x));
                    let corrected = *pixel - channel_correction - repeat_delta;
                    *pixel = if CLAMP {
                        corrected.clamp(0.0, 1.0)
                    } else {
                        corrected
                    };
                }
            }
        });
    pixels
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
    /// An X-Trans sensor's layout anchored at the full raw buffer's origin rather than the
    /// visible area's, which is what `cfa_type` holds.
    raw_xtrans_pattern: Option<XTransPattern>,
    camera_white_balance: Option<[f32; 4]>,
    iso: Option<u32>,
    /// Whether LibRaw's linearization curve is the identity up to `maximum`. A compressed format
    /// (Sony cRAW, Nikon lossy NEF, Canon C-RAW) maps its codes through a curve whose steps grow
    /// to several ADU in the highlights, so one ADU is no longer the quantization step.
    linear_curve: bool,
}

impl UnpackedRaw {
    /// Sensor ADU above black over `maximum − black`, from the file itself.
    fn sample_domain(&self) -> SampleDomain {
        SampleDomain {
            scale: f64::from(self.black_level.span()),
            origin: ScaleOrigin::Declared,
            pedestal: Pedestal::Removed,
            // No RAW format states a unit for sensor counts, and inventing one here would make a
            // RAW frame disagree with a FITS frame that spells the same thing differently.
            unit: None,
        }
    }

    /// One ADU's uniform-error σ in the normalized domain, or `None` when a compressed curve makes
    /// the step vary.
    fn quantization_sigma(&self) -> Option<f32> {
        self.linear_curve
            .then(|| QUANTIZATION_SIGMA_PER_STEP / self.black_level.span())
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

    /// Extract raw CFA pixels as normalized f32 (active area only).
    ///
    /// Applies channel and spatial black correction but NO white balance. `CLAMP`
    /// controls the `[0, 1]` floor/ceil (compile-time, like the kernels it
    /// dispatches to): `true` for monochrome **light** frames (the normalized
    /// output is the displayed image), `false` for the **calibration** path,
    /// where flooring at 0 would bias the stacked master dark/bias upward by
    /// clipping the sub-pedestal noise tail.
    fn extract_cfa_pixels<const CLAMP: bool>(&self) -> Result<Vec<f32>, ImageError> {
        let raw_data = self.raw_image_slice()?;

        let is_xtrans = matches!(self.cfa_type, Some(CfaType::XTrans(_)));
        let delta_channels = if is_xtrans {
            &self.black_level.channel_delta_norm[..3]
        } else {
            &self.black_level.channel_delta_norm
        };
        let has_delta = delta_channels
            .iter()
            .any(|&delta| delta.abs() > f32::EPSILON);
        let channel_delta = if !has_delta {
            None
        } else if let Some(CfaType::XTrans(visible_pattern)) = self.cfa_type {
            Some(ChannelBlackDelta::XTrans {
                visible_pattern,
                values: [
                    self.black_level.channel_delta_norm[0],
                    self.black_level.channel_delta_norm[1],
                    self.black_level.channel_delta_norm[2],
                ],
            })
        } else {
            Some(ChannelBlackDelta::LibRawFilter {
                visible_filters: self.visible_filters,
                values: self.black_level.channel_delta_norm,
            })
        };
        Ok(normalize_active_area::<CLAMP>(
            raw_data,
            self.layout,
            self.black_level.common,
            self.black_level.span,
            channel_delta,
            self.black_level.repeat.as_ref(),
        ))
    }

    /// Process Bayer sensor data using our fast SIMD demosaic. Returns planar
    /// `[R, G, B]` channels.
    fn demosaic_bayer(
        &self,
        visible_cfa_pattern: CfaPattern,
        cancel: &CancelToken,
    ) -> Result<[Vec<f32>; 3], ImageError> {
        let raw_data = self.raw_image_slice()?;

        // Pass 1: SIMD normalize with common black level
        let mut normalized_data =
            normalize_u16_to_f32_parallel(raw_data, self.black_level.common, self.black_level.span);

        apply_bayer_black_corrections(
            &mut normalized_data,
            self.layout.raw.width,
            self.layout.margin,
            self.visible_filters,
            &self.black_level.channel_delta_norm,
            self.black_level.repeat.as_ref(),
        );

        let raw_cfa_pattern =
            visible_cfa_pattern.at_raw_origin(self.layout.margin.y, self.layout.margin.x);
        let bayer = BayerImage::with_margins(&normalized_data, self.layout, raw_cfa_pattern);

        let demosaic_start = Instant::now();
        let mut rgb_pixels =
            rcd::demosaic(&bayer, cancel).map_err(|Cancelled| ImageError::cancelled(&self.path))?;
        let demosaic_elapsed = demosaic_start.elapsed();

        tracing::info!(
            "Fast SIMD demosaicing {}x{} took {:.2}ms",
            self.layout.active.width,
            self.layout.active.height,
            demosaic_elapsed.as_secs_f64() * 1000.0
        );

        clamp_interpolated(&mut rgb_pixels);
        Ok(rgb_pixels)
    }

    /// Process X-Trans sensor data using our Markesteijn demosaic. Returns planar `[R, G, B]`
    /// channels.
    ///
    /// Takes `self` by value so libraw's state — ~77 MB of it — is released before the demosaic
    /// allocates its own working set, and so the compiler, rather than the order of the lines in
    /// the caller, is what stops anything reaching a libraw instance this has already freed.
    fn demosaic_xtrans(
        self,
        raw_pattern: XTransPattern,
        cancel: &CancelToken,
    ) -> Result<[Vec<f32>; 3], ImageError> {
        // Copy raw u16 data so we can drop libraw before demosaicing.
        // P×2 bytes (~47 MB) instead of P×4 bytes (~93 MB) for normalized f32.
        let raw_u16: Vec<u16> = self.raw_image_slice()?.to_vec();

        let UnpackedRaw {
            libraw,
            path,
            layout,
            black_level,
            ..
        } = self;
        drop(libraw);

        // Convert 4-channel black to 3-channel for X-Trans (R=0, G=1, B=2)
        let channel_black = [
            black_level.per_channel[0],
            black_level.per_channel[1],
            black_level.per_channel[2],
        ];

        let mut pixels = xtrans::process_xtrans(
            &raw_u16,
            layout,
            raw_pattern,
            XTransNormalization {
                channel_black,
                span: black_level.span,
                black_repeat: black_level.repeat.as_ref(),
            },
            cancel,
        )
        .map_err(|Cancelled| ImageError::cancelled(&path))?;

        clamp_interpolated(&mut pixels);
        Ok(pixels)
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

        let pixels = if img_bits == 16 {
            // 16-bit data
            let expected_size = pixel_count
                .checked_mul(2)
                .expect("libraw: expected_size overflow");
            assert!(
                data_size >= expected_size,
                "libraw: data_size {data_size} < expected {expected_size}"
            );

            // SAFETY: data_ptr points to valid u16 data of the calculated size
            let data_u16 = unsafe { slice::from_raw_parts(data_ptr.cast::<u16>(), pixel_count) };

            // Normalize to 0.0-1.0
            data_u16
                .iter()
                .map(|&v| f32::from(v) / 65535.0)
                .collect::<Vec<f32>>()
        } else {
            // 8-bit data
            assert!(
                data_size >= pixel_count,
                "libraw: data_size {data_size} < expected {pixel_count}"
            );

            // SAFETY: data_ptr points to valid u8 data of the calculated size
            let data_u8 = unsafe { slice::from_raw_parts(data_ptr, pixel_count) };

            // Normalize to 0.0-1.0
            data_u8
                .iter()
                .map(|&v| f32::from(v) / 255.0)
                .collect::<Vec<f32>>()
        };

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
    let libraw = LibrawState::open(path)?;
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

    // SAFETY: inner is valid, color struct is initialized after unpack.
    let black_raw = unsafe { (*inner).color.black };
    let maximum_raw = unsafe { (*inner).color.maximum };

    // Get sensor info from libraw metadata
    // SAFETY: inner is valid, idata struct is initialized after unpack.
    let visible_filters = unsafe { (*inner).idata.filters };
    let cfa_type = sensor_cfa_type(&libraw, path)?;
    let raw_xtrans_pattern = match cfa_type {
        Some(CfaType::XTrans(_)) => {
            // SAFETY: inner is valid and LibRaw populates both patterns for X-Trans sensors.
            let raw_pattern = unsafe { (*inner).idata.xtrans_abs };
            Some(
                XTransPattern::new(xtrans_pattern_from_libraw(raw_pattern))
                    .map_err(|source| raw_err(path, source.to_string()))?,
            )
        }
        _ => None,
    };

    tracing::debug!(
        "libraw: filters=0x{:08x}, cfa_type={:?}",
        visible_filters,
        cfa_type
    );

    // Consolidate per-channel black levels (replicates libraw adjust_bl)
    // SAFETY: inner is valid, color.cblack is initialized after unpack.
    let cblack_raw: &[u32; 4104] = unsafe { &(*inner).color.cblack };
    let black_level = consolidate_black_levels(cblack_raw, black_raw, maximum_raw, visible_filters)
        .map_err(|source| raw_err(path, source.to_string()))?;

    // SAFETY: inner is valid, and color.cam_mul is initialized after unpack.
    let cam_mul = unsafe { (*inner).color.cam_mul };
    let camera_white_balance = canonical_camera_white_balance(cfa_type, cam_mul);
    let iso = extract_iso(inner);
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
        raw_xtrans_pattern,
        camera_white_balance,
        iso,
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

/// How a decoded frame's samples are laid out.
///
/// Our RGB demosaic kernels emit planar `[R, G, B]`, taken zero-copy into the image via
/// [`LinearImage::from_planar_channels`]. The mono path and libraw's fallback emit a single flat
/// buffer — grayscale or interleaved RGB — that [`LinearImage::from_pixels`] handles (grayscale
/// zero-copy, RGB de-interleaved).
#[derive(Debug)]
enum DemosaicedPixels {
    Planar([Vec<f32>; 3]),
    Flat(Vec<f32>),
}

/// One decoded raw frame before it becomes a [`LinearImage`]: the samples, the geometry they are
/// laid out by, and how they were produced.
///
/// Carries [`ImageDimensions`] rather than a loose size and channel count — every path here ends
/// in one, so building it at the source rejects a channel count the image type cannot hold at the
/// decode that produced it rather than several steps later.
#[derive(Debug)]
struct DecodedRawPreview {
    pixels: DemosaicedPixels,
    dimensions: ImageDimensions,
    color: ColorProvenance,
    demosaic: DemosaicProvenance,
}

/// Bring an interpolated RGB frame back inside the light-frame `[0, 1]` contract.
///
/// Interpolation is the only thing in the direct path that leaves the range: RAW input arrives
/// bounded from [`normalize_u16_to_f32_into`], but the demosaic kernels are shared with the
/// calibration path and deliberately pass samples through unclipped, so a step edge overshoots —
/// measurably ~1.06 out of RCD and ~1.16 out of Markesteijn. Clamping here rather than in the
/// kernels keeps the calibration path's sub-pedestal tail intact.
fn clamp_interpolated(planes: &mut [Vec<f32>; 3]) {
    for plane in planes {
        plane
            .par_iter_mut()
            .for_each(|sample| *sample = sample.clamp(0.0, 1.0));
    }
}

/// Load a raw file using libraw (C library, broader camera support).
///
/// Demosaicing strategy:
/// - Monochrome sensors: no demosaic needed, returns grayscale
/// - Known Bayer patterns (RGGB, BGGR, GRBG, GBRG): our RCD demosaic
/// - X-Trans: our Markesteijn demosaic
/// - Anything else — a linear DNG, sRAW or Foveon, or an exotic CFA: LibRaw's own processing
pub(crate) fn load_raw(path: &Path, context: &LoadContext) -> Result<LinearImage, ImageError> {
    context.check_cancelled(path)?;
    let raw = open_raw(path)?;
    context.check_cancelled(path)?;

    // Read before the match: the X-Trans arm consumes `raw` to free libraw ahead of its demosaic.
    let active = raw.layout.active;
    let iso = raw.iso;
    let camera_white_balance = raw.camera_white_balance;
    let domain = raw.sample_domain();

    let decoded = match raw.cfa_type {
        Some(CfaType::Mono) => {
            tracing::info!("Monochrome sensor detected, skipping demosaic");
            // Light frame: clamp to the [0, 1] display contract.
            let pixels = raw.extract_cfa_pixels::<true>()?;
            DecodedRawPreview {
                pixels: DemosaicedPixels::Flat(pixels),
                dimensions: ImageDimensions::new(active, 1),
                color: CfaType::Mono.demosaiced_color(),
                demosaic: CfaType::Mono.demosaic_provenance(),
            }
        }
        Some(cfa_type @ CfaType::Bayer(cfa_pattern)) => {
            tracing::debug!("Detected Bayer CFA pattern: {:?}", cfa_pattern);
            let planes = raw.demosaic_bayer(cfa_pattern, &context.cancel)?;
            DecodedRawPreview {
                pixels: DemosaicedPixels::Planar(planes),
                dimensions: ImageDimensions::new(active, 3),
                color: cfa_type.demosaiced_color(),
                demosaic: cfa_type.demosaic_provenance(),
            }
        }
        Some(cfa_type @ CfaType::XTrans(_)) => {
            tracing::info!("X-Trans sensor detected, using X-Trans demosaic");
            let raw_pattern = raw
                .raw_xtrans_pattern
                .expect("an X-Trans sensor's raw-origin layout is read when its file opens");
            let planes = raw.demosaic_xtrans(raw_pattern, &context.cancel)?;
            DecodedRawPreview {
                pixels: DemosaicedPixels::Planar(planes),
                dimensions: ImageDimensions::new(active, 3),
                color: cfa_type.demosaiced_color(),
                demosaic: cfa_type.demosaic_provenance(),
            }
        }
        None => {
            tracing::info!("no mosaic lumos demosaics; LibRaw processes the image");
            let demosaiced = raw.demosaic_libraw_fallback()?;
            context.check_cancelled(path)?;
            DecodedRawPreview {
                pixels: DemosaicedPixels::Flat(demosaiced.pixels),
                dimensions: demosaiced.dimensions,
                color: ColorProvenance::Unspecified,
                demosaic: DemosaicProvenance::LibRaw,
            }
        }
    };
    let DecodedRawPreview {
        pixels,
        dimensions,
        color,
        demosaic,
    } = decoded;

    let metadata = ImageMetadata {
        domain: Some(domain),
        iso,
        header_dimensions: vec![
            dimensions.height(),
            dimensions.width(),
            dimensions.channels(),
        ],
        camera_white_balance,
        provenance: Some(ImageProvenance {
            container: SourceContainer::CameraRaw,
            decoder: DecoderProvenance::LibRaw,
            transfer: TransferProvenance::RawNormalized,
            color,
            // Every arm above lands inside [0, 1]: the mono path clamps as it normalizes, libraw's
            // fallback divides by the integer maximum, and both demosaic paths clamp their output.
            clipped: true,
            demosaic,
            // libraw hands back the visible area top-down; no RAW format stores it otherwise.
            row_order: RowOrder::TopDown,
        }),
        ..Default::default()
    };

    let mut image = match pixels {
        DemosaicedPixels::Planar(planes) => LinearImage::from_planar_channels(dimensions, planes),
        DemosaicedPixels::Flat(px) => LinearImage::from_pixels(dimensions, px),
    };
    image.metadata = metadata;
    Ok(image)
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
        // A sensor reports a value for every photosite; no RAW format has an undefined-sample
        // convention to decode, so this is settled rather than conservative.
        may_carry_nulls: false,
    })
}

/// Load raw file and return un-demosaiced CFA data.
///
/// Returns single-channel f32 data with CFA pattern metadata.
/// Used for calibration frame processing (darks, flats, bias)
/// where hot pixel correction must happen before demosaicing.
pub(crate) fn load_raw_cfa(path: &Path, context: &LoadContext) -> Result<CfaImage, ImageError> {
    context.check_cancelled(path)?;
    let raw = open_raw(path)?;
    context.check_cancelled(path)?;

    let cfa_type = raw.cfa_type.ok_or_else(|| not_a_cfa_frame(path))?;

    // Calibration path: keep signed, un-clamped values so stacked master
    // dark/bias means aren't biased upward by clipping the sub-pedestal tail.
    let pixels = raw.extract_cfa_pixels::<false>()?;
    let metadata = ImageMetadata {
        domain: Some(raw.sample_domain()),
        quantization_sigma: raw.quantization_sigma(),
        iso: raw.iso,
        header_dimensions: vec![raw.layout.active.height, raw.layout.active.width, 1],
        camera_white_balance: raw.camera_white_balance,
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
        data: Buffer2::new(raw.layout.active.width, raw.layout.active.height, pixels),
        cfa_type,
        metadata,
        // A sensor reports a value for every photosite; no RAW format has an undefined-sample
        // convention to decode.
        nulls: None,
    })
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
