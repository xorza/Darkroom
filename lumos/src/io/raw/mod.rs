pub(crate) mod demosaic;
mod normalize;

#[cfg(test)]
mod benches;
#[cfg(test)]
mod tests;

use libraw_sys as sys;
#[cfg(unix)]
use std::ffi::CString;
#[cfg(not(unix))]
use std::fs;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::slice;
use std::time::Instant;

use crate::io::image::error::ImageError;

use rayon::prelude::*;

use crate::io::image::cfa::{CfaImage, CfaType, QUANTIZATION_SIGMA_PER_STEP};
use crate::io::image::linear::LinearImage;
use crate::io::image::sensor::{SensorType, detect_sensor_type};
use crate::io::image::{
    BitPix, ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageDimensions, ImageMetadata,
    ImageProvenance, SourceContainer, TransferProvenance,
};
use crate::math::vec2us::Vec2us;
use common::CancelToken;
use demosaic::bayer::{BayerImage, CfaPattern, demosaic_bayer};
use demosaic::xtrans;
use imaginarium::Buffer2;

use normalize::{normalize_u16_to_f32_into, normalize_u16_to_f32_parallel};

/// Camera-RAW extensions accepted by this decoder.
pub const RAW_EXTENSIONS: &[&str] = &["raf", "cr2", "cr3", "nef", "arw", "dng"];

/// Allocate a Vec of given length without zeroing.
///
/// SAFETY: Caller must ensure every element is written before it's read.
/// This avoids expensive kernel page zeroing (clear_page_erms) for large buffers.
#[allow(clippy::uninit_vec)]
pub(crate) unsafe fn alloc_uninit_vec<T>(len: usize) -> Vec<T> {
    let mut v = Vec::with_capacity(len);
    unsafe { v.set_len(len) };
    v
}

/// RAII guard for libraw_data_t to ensure proper cleanup.
#[derive(Debug)]
struct LibrawGuard(*mut sys::libraw_data_t);

// SAFETY: LibrawGuard owns the pointer and ensures exclusive access.
// The libraw library itself is thread-safe when using separate instances.
unsafe impl Send for LibrawGuard {}

impl Drop for LibrawGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: We own this pointer and it was allocated by libraw_init.
            unsafe { sys::libraw_close(self.0) };
        }
    }
}

/// RAII guard for libraw_processed_image_t to ensure proper cleanup.
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
    width: usize,
    height: usize,
    delta_norm: Box<[f32]>,
}

impl BlackRepeat {
    #[inline(always)]
    fn at_visible(&self, row: usize, col: usize) -> f32 {
        self.delta_norm[(row % self.height) * self.width + col % self.width]
    }

    #[inline(always)]
    fn at_raw(&self, raw_row: usize, raw_col: usize, top_margin: usize, left_margin: usize) -> f32 {
        // LibRaw defines repeat phase after cropping, so margins shift full-buffer coordinates.
        let row = (raw_row % self.height + self.height - top_margin % self.height) % self.height;
        let col = (raw_col % self.width + self.width - left_margin % self.width) % self.width;
        self.delta_norm[row * self.width + col]
    }
}

#[derive(Debug)]
struct BlackLevel {
    per_channel: [f32; 4],
    common: f32,
    inv_range: f32,
    channel_delta_norm: [f32; 4],
    repeat: Option<BlackRepeat>,
}

/// Replicate libraw's `adjust_bl()` black-level consolidation.
///
/// See libraw `utils_libraw.cpp:464-540` for the reference C++ implementation.
fn consolidate_black_levels(
    cblack_raw: &[u32; 4104],
    black_raw: u32,
    maximum_raw: u32,
    visible_filters: u32,
) -> Result<BlackLevel, String> {
    let mut cblack = [0u32; 4104];
    cblack.copy_from_slice(cblack_raw);
    let mut black = black_raw;

    if cblack[4] > 0 && cblack[5] > 0 {
        let pattern_size = (cblack[4] as usize)
            .checked_mul(cblack[5] as usize)
            .ok_or_else(|| {
                format!(
                    "invalid spatial black pattern dimensions: {}x{}",
                    cblack[5], cblack[4]
                )
            })?;
        if pattern_size > cblack.len() - 6 {
            return Err(format!(
                "spatial black pattern {}x{} exceeds {} entries",
                cblack[5],
                cblack[4],
                cblack.len() - 6
            ));
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
        return Err(format!(
            "invalid black level: common black {common} >= maximum {maximum_raw}"
        ));
    }
    let inv_range = 1.0 / effective_max;
    let mut channel_delta_norm = [0f32; 4];
    for c in 0..4 {
        channel_delta_norm[c] = (per_channel[c] - common) * inv_range;
    }
    let repeat = if cblack[4] > 0 && cblack[5] > 0 {
        let height = cblack[4] as usize;
        let width = cblack[5] as usize;
        let pattern_size = height * width;
        Some(BlackRepeat {
            width,
            height,
            delta_norm: cblack[6..6 + pattern_size]
                .iter()
                .map(|&delta| delta as f32 * inv_range)
                .collect(),
        })
    } else {
        None
    };

    tracing::debug!(
        "Black levels: common={common}, per_channel={per_channel:?}, \
         delta_norm={channel_delta_norm:?}, repeat={}x{}, inv_range={inv_range}",
        repeat.as_ref().map_or(0, |pattern| pattern.width),
        repeat.as_ref().map_or(0, |pattern| pattern.height)
    );

    Ok(BlackLevel {
        per_channel,
        common,
        inv_range,
        channel_delta_norm,
        repeat,
    })
}

fn canonical_camera_white_balance(sensor_type: &SensorType, cam_mul: [f32; 4]) -> Option<[f32; 4]> {
    if matches!(sensor_type, SensorType::Monochrome) {
        return None;
    }

    let mut multipliers = cam_mul;
    if matches!(sensor_type, SensorType::XTrans) || multipliers[3] == 0.0 {
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
    top_margin: usize,
    left_margin: usize,
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
                let ch = raw_filter_color(visible_filters, row, col, top_margin, left_margin);
                let repeat_delta = repeat.map_or(0.0, |pattern| {
                    pattern.at_raw(row, col, top_margin, left_margin)
                });
                *pixel = (*pixel - delta_norm[ch] - repeat_delta).clamp(0.0, 1.0);
            }
        });
}

/// Libraw FC macro: determine color channel at (row, col) from filters bitmask.
#[inline(always)]
fn libraw_filter_color(filters: u32, row: usize, col: usize) -> usize {
    ((filters >> (((row << 1 & 14) | (col & 1)) << 1)) & 3) as usize
}

#[inline(always)]
fn raw_filter_color(
    visible_filters: u32,
    raw_row: usize,
    raw_col: usize,
    top_margin: usize,
    left_margin: usize,
) -> usize {
    libraw_filter_color(
        visible_filters,
        raw_row.wrapping_sub(top_margin),
        raw_col.wrapping_sub(left_margin),
    )
}

#[allow(clippy::unnecessary_cast)]
fn xtrans_pattern_from_libraw(pattern: [[std::ffi::c_char; 6]; 6]) -> [[u8; 6]; 6] {
    pattern.map(|row| row.map(|color| color as u8))
}

pub(crate) fn raw_err(path: &Path, reason: impl Into<String>) -> ImageError {
    ImageError::Raw {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

fn validate_xtrans_pattern(path: &Path, pattern: [[u8; 6]; 6]) -> Result<(), ImageError> {
    xtrans::XTransPattern::new(pattern).map_err(|source| raw_err(path, source.to_string()))?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct RawActiveArea {
    raw_width: usize,
    width: usize,
    height: usize,
    top_margin: usize,
    left_margin: usize,
}

#[derive(Debug, Clone, Copy)]
enum ChannelBlackDelta {
    LibRawFilter {
        visible_filters: u32,
        values: [f32; 4],
    },
    XTrans {
        visible_pattern: [[u8; 6]; 6],
        values: [f32; 3],
    },
}

impl ChannelBlackDelta {
    #[inline(always)]
    fn at_visible(&self, row: usize, col: usize) -> f32 {
        match self {
            ChannelBlackDelta::LibRawFilter {
                visible_filters,
                values,
            } => values[libraw_filter_color(*visible_filters, row, col)],
            ChannelBlackDelta::XTrans {
                visible_pattern,
                values,
            } => values[visible_pattern[row % 6][col % 6] as usize],
        }
    }
}

fn normalize_active_area<const CLAMP: bool>(
    raw_data: &[u16],
    area: RawActiveArea,
    black: f32,
    inv_range: f32,
    channel_delta: Option<ChannelBlackDelta>,
    repeat: Option<&BlackRepeat>,
) -> Vec<f32> {
    let output_size = area.width * area.height;
    // SAFETY: Every element is written by the parallel row pass below.
    let mut pixels = unsafe { alloc_uninit_vec::<f32>(output_size) };
    pixels
        .par_chunks_mut(area.width)
        .enumerate()
        .for_each(|(y, row)| {
            let raw_y = area.top_margin + y;
            let src_start = raw_y * area.raw_width + area.left_margin;
            let source = &raw_data[src_start..src_start + area.width];
            normalize_u16_to_f32_into::<CLAMP>(source, row, black, inv_range);

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
    inner: *mut sys::libraw_data_t,
    guard: Option<LibrawGuard>,
    buf: Option<Vec<u8>>,
    path: PathBuf,
    raw_width: usize,
    raw_height: usize,
    width: usize,
    height: usize,
    top_margin: usize,
    left_margin: usize,
    black_level: BlackLevel,
    visible_filters: u32,
    sensor_type: SensorType,
    camera_white_balance: Option<[f32; 4]>,
    iso: Option<u32>,
}

impl UnpackedRaw {
    /// Get the raw u16 image pointer and total pixel count.
    /// Returns the pointer and count, or an error if null.
    fn raw_image_slice(&self) -> Result<&[u16], ImageError> {
        // SAFETY: inner is valid and unpack succeeded.
        let raw_image_ptr = unsafe { (*self.inner).rawdata.raw_image };
        if raw_image_ptr.is_null() {
            return Err(raw_err(&self.path, "libraw: raw_image is null"));
        }

        let pixel_count = self
            .raw_width
            .checked_mul(self.raw_height)
            .expect("libraw: raw dimensions overflow");

        // SAFETY: raw_image_ptr is valid (checked above), and dimensions were validated in open_raw.
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

        let delta_channels = if matches!(self.sensor_type, SensorType::XTrans) {
            &self.black_level.channel_delta_norm[..3]
        } else {
            &self.black_level.channel_delta_norm
        };
        let has_delta = delta_channels
            .iter()
            .any(|&delta| delta.abs() > f32::EPSILON);
        let channel_delta = if !has_delta {
            None
        } else if matches!(self.sensor_type, SensorType::XTrans) {
            Some(ChannelBlackDelta::XTrans {
                visible_pattern: self.visible_xtrans_pattern(),
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
            RawActiveArea {
                raw_width: self.raw_width,
                width: self.width,
                height: self.height,
                top_margin: self.top_margin,
                left_margin: self.left_margin,
            },
            self.black_level.common,
            self.black_level.inv_range,
            channel_delta,
            self.black_level.repeat.as_ref(),
        ))
    }

    /// Process Bayer sensor data using our fast SIMD demosaic. Returns planar
    /// `[R, G, B]` channels.
    fn demosaic_bayer(&self, visible_cfa_pattern: CfaPattern) -> Result<[Vec<f32>; 3], ImageError> {
        let raw_data = self.raw_image_slice()?;

        // Pass 1: SIMD normalize with common black level
        let mut normalized_data = normalize_u16_to_f32_parallel(
            raw_data,
            self.black_level.common,
            self.black_level.inv_range,
        );

        apply_bayer_black_corrections(
            &mut normalized_data,
            self.raw_width,
            self.top_margin,
            self.left_margin,
            self.visible_filters,
            &self.black_level.channel_delta_norm,
            self.black_level.repeat.as_ref(),
        );

        let raw_cfa_pattern = visible_cfa_pattern.at_raw_origin(self.top_margin, self.left_margin);
        let bayer = BayerImage::with_margins(
            &normalized_data,
            self.raw_width,
            self.raw_height,
            self.width,
            self.height,
            self.top_margin,
            self.left_margin,
            raw_cfa_pattern,
        );

        let demosaic_start = Instant::now();
        // The u16 decode path isn't cancellable — a never-token can't yield `Cancelled`.
        let rgb_pixels = demosaic_bayer(&bayer, &CancelToken::never())
            .expect("never-token demosaic cannot be cancelled");
        let demosaic_elapsed = demosaic_start.elapsed();

        tracing::info!(
            "Fast SIMD demosaicing {}x{} took {:.2}ms",
            self.width,
            self.height,
            demosaic_elapsed.as_secs_f64() * 1000.0
        );

        Ok(rgb_pixels)
    }

    /// Extract LibRaw's visible-origin X-Trans pattern for active-area consumers.
    fn visible_xtrans_pattern(&self) -> [[u8; 6]; 6] {
        // SAFETY: inner is valid and xtrans is populated for X-Trans sensors.
        let pattern = unsafe { (*self.inner).idata.xtrans };
        xtrans_pattern_from_libraw(pattern)
    }

    /// Extract LibRaw's absolute X-Trans pattern for full-raw-buffer consumers.
    fn raw_xtrans_pattern(&self) -> [[u8; 6]; 6] {
        // SAFETY: inner is valid and xtrans_abs is populated for X-Trans sensors.
        let pattern = unsafe { (*self.inner).idata.xtrans_abs };
        xtrans_pattern_from_libraw(pattern)
    }

    /// Process X-Trans sensor data using our Markesteijn demosaic.
    ///
    /// Drops LibRaw state and any fallback input buffer before the expensive demosaicing step,
    /// reducing peak memory by ~77 MB.
    fn demosaic_xtrans(&mut self, raw_pattern: [[u8; 6]; 6]) -> Result<[Vec<f32>; 3], ImageError> {
        let raw_data = self.raw_image_slice()?;

        // Copy raw u16 data so we can drop libraw before demosaicing.
        // P×2 bytes (~47 MB) instead of P×4 bytes (~93 MB) for normalized f32.
        let raw_u16: Vec<u16> = raw_data.to_vec();

        // Convert 4-channel black to 3-channel for X-Trans (R=0, G=1, B=2)
        let bl = &self.black_level;
        let channel_black = [bl.per_channel[0], bl.per_channel[1], bl.per_channel[2]];

        // Drop libraw and any fallback file buffer to reduce peak memory during demosaicing
        self.guard.take();
        self.buf.take();

        let pixels = xtrans::process_xtrans(
            &raw_u16,
            self.raw_width,
            self.raw_height,
            self.width,
            self.height,
            self.top_margin,
            self.left_margin,
            raw_pattern,
            channel_black,
            bl.inv_range,
            bl.repeat.as_ref(),
        )
        .map_err(|source| raw_err(&self.path, source.to_string()))?;

        Ok(pixels)
    }

    /// Process unknown CFA pattern using libraw's built-in demosaic.
    /// This is slower but handles exotic sensor patterns correctly.
    /// Returns (pixels, width, height, num_channels).
    fn demosaic_libraw_fallback(&self) -> Result<(Vec<f32>, usize, usize, usize), ImageError> {
        let demosaic_start = Instant::now();

        // Configure libraw for linear output (no gamma, no color conversion)
        // SAFETY: inner is valid
        unsafe {
            // Output in linear color space (no gamma curve)
            (*self.inner).params.gamm[0] = 1.0;
            (*self.inner).params.gamm[1] = 1.0;
            // No brightness adjustment
            (*self.inner).params.bright = 1.0;
            // LibRaw otherwise falls back to daylight WB when camera and auto WB are disabled.
            (*self.inner).params.user_mul = [1.0; 4];
            (*self.inner).params.use_auto_wb = 0;
            (*self.inner).params.use_camera_wb = 0;
            // Output 16-bit
            (*self.inner).params.output_bps = 16;
            // Linear color space (raw)
            (*self.inner).params.output_color = 0;
            // No auto-brightness
            (*self.inner).params.no_auto_bright = 1;
        }

        // Run libraw's demosaic
        // SAFETY: inner is valid and configured
        let ret = unsafe { sys::libraw_dcraw_process(self.inner) };
        if ret != 0 {
            return Err(raw_err(
                &self.path,
                format!("libraw: dcraw_process failed, error code: {}", ret),
            ));
        }

        // Get the processed image
        // SAFETY: inner is valid and dcraw_process succeeded
        let mut errc: i32 = 0;
        let processed_ptr = unsafe { sys::libraw_dcraw_make_mem_image(self.inner, &mut errc) };
        if processed_ptr.is_null() || errc != 0 {
            return Err(raw_err(
                &self.path,
                format!("libraw: dcraw_make_mem_image failed, error code: {}", errc),
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
                "libraw: data_size {} < expected {}",
                data_size,
                expected_size
            );

            // SAFETY: data_ptr points to valid u16 data of the calculated size
            let data_u16 = unsafe { slice::from_raw_parts(data_ptr as *const u16, pixel_count) };

            // Normalize to 0.0-1.0
            data_u16
                .iter()
                .map(|&v| (v as f32) / 65535.0)
                .collect::<Vec<f32>>()
        } else {
            // 8-bit data
            assert!(
                data_size >= pixel_count,
                "libraw: data_size {} < expected {}",
                data_size,
                pixel_count
            );

            // SAFETY: data_ptr points to valid u8 data of the calculated size
            let data_u8 = unsafe { slice::from_raw_parts(data_ptr, pixel_count) };

            // Normalize to 0.0-1.0
            data_u8
                .iter()
                .map(|&v| (v as f32) / 255.0)
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

        Ok((pixels, img_width, img_height, img_colors))
    }
}

/// Open and unpack a raw file using libraw.
///
/// Performs: libraw init, file open, unpack, dimension/color
/// validation, sensor type detection, and ISO extraction.
fn open_raw(path: &Path) -> Result<UnpackedRaw, ImageError> {
    // SAFETY: libraw_init returns a valid pointer or null on failure.
    let inner = unsafe { sys::libraw_init(0) };
    if inner.is_null() {
        return Err(raw_err(path, "libraw: Failed to initialize"));
    }

    // Guard ensures cleanup even on early return or panic
    let guard = LibrawGuard(inner);

    let buf = open_libraw_input(inner, path)?;

    // SAFETY: inner is valid and open_buffer succeeded.
    let ret = unsafe { sys::libraw_unpack(inner) };
    if ret != 0 {
        return Err(raw_err(
            path,
            format!("libraw: Failed to unpack, error code: {}", ret),
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
            format!(
                "libraw: Invalid raw dimensions: {}x{}",
                raw_width, raw_height
            ),
        ));
    }
    if width == 0 || height == 0 {
        return Err(raw_err(
            path,
            format!("libraw: Invalid output dimensions: {}x{}", width, height),
        ));
    }
    if top_margin + height > raw_height || left_margin + width > raw_width {
        return Err(raw_err(
            path,
            format!(
                "libraw: Margins exceed raw dimensions: margins ({}, {}) + size ({}, {}) > raw ({}, {})",
                top_margin, left_margin, width, height, raw_width, raw_height
            ),
        ));
    }

    // SAFETY: inner is valid, color struct is initialized after unpack.
    let black_raw = unsafe { (*inner).color.black } as u32;
    let maximum_raw = unsafe { (*inner).color.maximum } as u32;

    // Get sensor info from libraw metadata
    // SAFETY: inner is valid, idata struct is initialized after unpack.
    let visible_filters = unsafe { (*inner).idata.filters };
    let colors = unsafe { (*inner).idata.colors };
    let sensor_type = detect_sensor_type(visible_filters, colors);
    if matches!(sensor_type, SensorType::XTrans) {
        // SAFETY: inner is valid and LibRaw populates both patterns for X-Trans sensors.
        let visible_pattern = unsafe { (*inner).idata.xtrans };
        validate_xtrans_pattern(path, xtrans_pattern_from_libraw(visible_pattern))?;
        // SAFETY: same initialized LibRaw metadata as the visible-origin pattern above.
        let raw_pattern = unsafe { (*inner).idata.xtrans_abs };
        validate_xtrans_pattern(path, xtrans_pattern_from_libraw(raw_pattern))?;
    }

    tracing::debug!(
        "libraw: filters=0x{:08x}, colors={}, sensor_type={:?}",
        visible_filters,
        colors,
        sensor_type
    );

    // Consolidate per-channel black levels (replicates libraw adjust_bl)
    // SAFETY: inner is valid, color.cblack is initialized after unpack.
    let cblack_raw: [u32; 4104] = unsafe { (*inner).color.cblack };
    let black_level =
        consolidate_black_levels(&cblack_raw, black_raw, maximum_raw, visible_filters)
            .map_err(|reason| raw_err(path, reason))?;

    // SAFETY: inner is valid, and color.cam_mul is initialized after unpack.
    let cam_mul = unsafe { (*inner).color.cam_mul };
    let camera_white_balance = canonical_camera_white_balance(&sensor_type, cam_mul);
    let iso = extract_iso(inner);

    Ok(UnpackedRaw {
        inner,
        guard: Some(guard),
        buf,
        path: path.to_path_buf(),
        raw_width,
        raw_height,
        width,
        height,
        top_margin,
        left_margin,
        black_level,
        visible_filters,
        sensor_type,
        camera_white_balance,
        iso,
    })
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
        return Err(raw_err(
            path,
            format!("libraw: Failed to open file, error code: {ret}"),
        ));
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
        return Err(raw_err(
            path,
            format!("libraw: Failed to open buffer, error code: {ret}"),
        ));
    }
    Ok(Some(buf))
}

/// Load raw file using libraw (C library, broader camera support).
///
/// Demosaicing strategy:
/// - Monochrome sensors: no demosaic needed, returns grayscale
/// - Known Bayer patterns (RGGB, BGGR, GRBG, GBRG): fast SIMD demosaic
/// - Unknown patterns (X-Trans, etc.): libraw's built-in demosaic (slower but correct)
///
/// Our RGB demosaic kernels emit planar `[R, G, B]`, taken zero-copy into the
/// image via [`LinearImage::from_planar_channels`]. The mono path and libraw's
/// fallback emit a single flat buffer — grayscale or interleaved RGB — that
/// [`LinearImage::from_pixels`] handles (grayscale zero-copy, RGB de-interleaved).
#[derive(Debug)]
enum DemosaicedPixels {
    Planar([Vec<f32>; 3]),
    Flat(Vec<f32>),
}

#[derive(Debug)]
struct DecodedRawPreview {
    pixels: DemosaicedPixels,
    width: usize,
    height: usize,
    channels: usize,
    cfa_type: Option<CfaType>,
    color: ColorProvenance,
    demosaic: DemosaicProvenance,
}

fn clamp_direct_raw_image(image: &mut LinearImage) {
    for channel in 0..image.channels() {
        image
            .channel_mut(channel)
            .pixels_mut()
            .par_iter_mut()
            .for_each(|sample| *sample = sample.clamp(0.0, 1.0));
    }
}

pub(crate) fn load_raw(path: &Path) -> Result<LinearImage, ImageError> {
    let mut raw = open_raw(path)?;

    let sensor_type = raw.sensor_type.clone();
    let decoded = match sensor_type {
        SensorType::Monochrome => {
            tracing::info!("Monochrome sensor detected, skipping demosaic");
            // Light frame: clamp to the [0, 1] display contract.
            let pixels = raw.extract_cfa_pixels::<true>()?;
            DecodedRawPreview {
                pixels: DemosaicedPixels::Flat(pixels),
                width: raw.width,
                height: raw.height,
                channels: 1,
                cfa_type: None,
                color: ColorProvenance::Monochrome,
                demosaic: DemosaicProvenance::None,
            }
        }
        SensorType::Bayer(cfa_pattern) => {
            tracing::debug!("Detected Bayer CFA pattern: {:?}", cfa_pattern);
            let planes = raw.demosaic_bayer(cfa_pattern)?;
            DecodedRawPreview {
                pixels: DemosaicedPixels::Planar(planes),
                width: raw.width,
                height: raw.height,
                channels: 3,
                cfa_type: Some(CfaType::Bayer(cfa_pattern)),
                color: ColorProvenance::SensorRgb,
                demosaic: DemosaicProvenance::LumosRcd,
            }
        }
        SensorType::XTrans => {
            tracing::info!("X-Trans sensor detected, using X-Trans demosaic");
            let visible_pattern = raw.visible_xtrans_pattern();
            let raw_pattern = raw.raw_xtrans_pattern();
            let planes = raw.demosaic_xtrans(raw_pattern)?;
            DecodedRawPreview {
                pixels: DemosaicedPixels::Planar(planes),
                width: raw.width,
                height: raw.height,
                channels: 3,
                cfa_type: Some(CfaType::XTrans(visible_pattern)),
                color: ColorProvenance::SensorRgb,
                demosaic: DemosaicProvenance::LumosMarkesteijn,
            }
        }
        SensorType::Unknown => {
            tracing::info!("Unknown CFA pattern, using libraw demosaic fallback");
            let (pixels, w, h, c) = raw.demosaic_libraw_fallback()?;
            DecodedRawPreview {
                pixels: DemosaicedPixels::Flat(pixels),
                width: w,
                height: h,
                channels: c,
                cfa_type: Some(CfaType::Mono),
                color: ColorProvenance::Unspecified,
                demosaic: DemosaicProvenance::LibRaw,
            }
        }
    };
    let DecodedRawPreview {
        pixels,
        width,
        height,
        channels,
        cfa_type,
        color,
        demosaic,
    } = decoded;

    let metadata = ImageMetadata {
        iso: raw.iso,
        bitpix: BitPix::UInt16,
        header_dimensions: vec![height, width, channels],
        cfa_type,
        camera_white_balance: raw.camera_white_balance,
        provenance: Some(ImageProvenance {
            container: SourceContainer::CameraRaw,
            decoder: DecoderProvenance::LibRaw,
            transfer: TransferProvenance::RawNormalized,
            color,
            clipped: true,
            demosaic,
        }),
        ..Default::default()
    };
    drop(raw);

    let dimensions = ImageDimensions::new((width, height), channels);
    let mut image = match pixels {
        DemosaicedPixels::Planar(planes) => LinearImage::from_planar_channels(dimensions, planes),
        DemosaicedPixels::Flat(px) => LinearImage::from_pixels(dimensions, px),
    };
    clamp_direct_raw_image(&mut image);
    image.metadata = metadata;
    Ok(image)
}

/// Load raw file and return un-demosaiced CFA data.
///
/// Returns single-channel f32 data with CFA pattern metadata.
/// Used for calibration frame processing (darks, flats, bias)
/// where hot pixel correction must happen before demosaicing.
///
/// Read just the output pixel dimensions of a RAW file from its header — opening through LibRaw
/// without the expensive `libraw_unpack`. Returns the same `width × height` [`load_raw_cfa`]
/// produces, so `w · h · size_of::<f32>()` is the in-memory CFA frame footprint. Cheap enough to
/// size a memory budget before committing to full decodes.
pub(crate) fn raw_dimensions(path: &Path) -> Result<Vec2us, ImageError> {
    // SAFETY: libraw_init returns a valid pointer or null on failure.
    let inner = unsafe { sys::libraw_init(0) };
    if inner.is_null() {
        return Err(raw_err(path, "libraw: Failed to initialize"));
    }
    // Drops (frees libraw state) on every return path.
    let _guard = LibrawGuard(inner);

    // Retain the fallback buffer until after metadata is read on platforms where the path cannot
    // be passed losslessly through LibRaw's narrow file API.
    let _buf = open_libraw_input(inner, path)?;

    // SAFETY: opening succeeded, so the sizes struct is initialized.
    let width = unsafe { (*inner).sizes.width } as usize;
    let height = unsafe { (*inner).sizes.height } as usize;
    if width == 0 || height == 0 {
        return Err(raw_err(
            path,
            format!("libraw: Invalid output dimensions: {width}x{height}"),
        ));
    }
    Ok(Vec2us::new(width, height))
}

pub(crate) fn load_raw_cfa(path: &Path) -> Result<CfaImage, ImageError> {
    let raw = open_raw(path)?;

    let cfa_type = match &raw.sensor_type {
        SensorType::Monochrome => CfaType::Mono,
        SensorType::Bayer(p) => CfaType::Bayer(*p),
        SensorType::XTrans => CfaType::XTrans(raw.visible_xtrans_pattern()),
        SensorType::Unknown => {
            return Err(raw_err(
                &raw.path,
                "raw CFA extraction is unsupported for unknown sensor types",
            ));
        }
    };

    // Calibration path: keep signed, un-clamped values so stacked master
    // dark/bias means aren't biased upward by clipping the sub-pedestal tail.
    let pixels = raw.extract_cfa_pixels::<false>()?;
    let metadata = ImageMetadata {
        iso: raw.iso,
        bitpix: BitPix::UInt16,
        header_dimensions: vec![raw.height, raw.width, 1],
        cfa_type: Some(cfa_type),
        camera_white_balance: raw.camera_white_balance,
        provenance: Some(ImageProvenance {
            container: SourceContainer::CameraRaw,
            decoder: DecoderProvenance::LibRaw,
            transfer: TransferProvenance::RawNormalized,
            color: ColorProvenance::SensorCfa,
            clipped: false,
            demosaic: DemosaicProvenance::None,
        }),
        ..Default::default()
    };

    Ok(CfaImage {
        data: Buffer2::new(raw.width, raw.height, pixels),
        metadata,
        quantization_sigma: Some(raw.black_level.inv_range * QUANTIZATION_SIGMA_PER_STEP),
    })
}

/// Extract ISO from libraw metadata.
fn extract_iso(inner: *mut sys::libraw_data_t) -> Option<u32> {
    // SAFETY: inner is valid after unpack.
    let iso_speed = unsafe { (*inner).other.iso_speed };
    if iso_speed > 0.0 {
        Some(iso_speed.round() as u32)
    } else {
        None
    }
}
