pub mod demosaic;
mod normalize;

#[cfg(test)]
mod benches;
#[cfg(test)]
mod tests;

use libraw_sys as sys;
use std::fs;
use std::path::{Path, PathBuf};
use std::slice;
use std::time::Instant;

use crate::astro_image::error::ImageError;

use rayon::prelude::*;

use crate::astro_image::cfa::{CfaImage, CfaType};
use crate::astro_image::sensor::{SensorType, detect_sensor_type};
use crate::astro_image::{AstroImage, AstroImageMetadata, BitPix, ImageDimensions};
use crate::common::Buffer2;
use demosaic::xtrans::process_xtrans;
use demosaic::{BayerImage, CfaPattern, demosaic_bayer};

use normalize::normalize_u16_to_f32_parallel;

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

/// Per-channel black levels after consolidation.
///
/// Replicates libraw's `adjust_bl()` logic: folds spatial patterns into per-channel
/// values, extracts the common minimum, and computes normalized deltas for efficient
/// two-pass correction (SIMD uniform pass + per-pixel channel delta pass).
#[derive(Debug, Clone)]
struct BlackLevel {
    /// Per-channel total black [R, G1, B, G2] (common + per-channel delta).
    per_channel: [f32; 4],
    /// Common (minimum) black across all channels. Used for SIMD first pass.
    common: f32,
    /// 1.0 / (maximum - common).
    inv_range: f32,
    /// `(per_channel[c] - common) * inv_range` — normalized delta per channel.
    /// Applied in the second per-pixel pass.
    channel_delta_norm: [f32; 4],
}

/// Replicate libraw's `adjust_bl()` to consolidate per-channel and spatial
/// black level corrections into a single per-channel array.
///
/// See libraw `utils_libraw.cpp:464-540` for the reference C++ implementation.
fn consolidate_black_levels(
    cblack_raw: &[u32; 4104],
    black_raw: u32,
    maximum_raw: u32,
    filters: u32,
) -> BlackLevel {
    let mut cblack = [0u32; 4104];
    cblack.copy_from_slice(cblack_raw);
    let mut black = black_raw;

    // Step 1: Fold spatial pattern into per-channel values.
    // For Bayer sensors with ~2x2 spatial pattern:
    if filters > 1000 && cblack[4].div_ceil(2) == 1 && cblack[5].div_ceil(2) == 1 {
        // Map position (c/2, c%2) to color channel using FC macro.
        // FC(row,col) = (filters >> (((row<<1 & 14) | (col & 1)) << 1)) & 3
        let mut clrs = [0u32; 4];
        let mut last_g: Option<usize> = None;
        let mut g_count = 0;
        for (c, clr) in clrs.iter_mut().enumerate() {
            let row = c / 2;
            let col = c % 2;
            *clr = (filters >> (((row << 1 & 14) | (col & 1)) << 1)) & 3;
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
    } else if filters <= 1000 && cblack[4] == 1 && cblack[5] == 1 {
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

    // Warn if spatial pattern still present after consolidation
    if cblack[4] > 0 && cblack[5] > 0 {
        tracing::warn!(
            "Unhandled spatial black pattern: {}x{} (using per-channel only)",
            cblack[4],
            cblack[5]
        );
    }

    // Step 4: Final per-channel = cblack[c] + black
    let mut per_channel = [0f32; 4];
    for c in 0..4 {
        per_channel[c] = (cblack[c] + black) as f32;
    }
    let common = black as f32;
    let effective_max = maximum_raw as f32 - common;
    assert!(
        effective_max > 0.0,
        "Invalid black level: common={common}, maximum={maximum_raw}"
    );
    let inv_range = 1.0 / effective_max;
    let mut channel_delta_norm = [0f32; 4];
    for c in 0..4 {
        channel_delta_norm[c] = (per_channel[c] - common) * inv_range;
    }

    tracing::debug!(
        "Black levels: common={common}, per_channel={per_channel:?}, \
         delta_norm={channel_delta_norm:?}, inv_range={inv_range}"
    );

    BlackLevel {
        per_channel,
        common,
        inv_range,
        channel_delta_norm,
    }
}

/// Compute normalized WB multipliers from camera multipliers.
///
/// Normalizes so the smallest multiplier is 1.0 (avoids clipping).
/// Returns None if cam_mul appears invalid (all zeros or non-finite).
fn compute_wb_multipliers(cam_mul: [f32; 4]) -> Option<[f32; 4]> {
    let mut mul = cam_mul;
    // cam_mul[3] may be 0 for 3-color cameras; use cam_mul[1] (green) as fallback
    if mul[3] == 0.0 {
        mul[3] = mul[1];
    }

    // Validate: all must be positive and finite
    if mul.iter().any(|&m| m <= 0.0 || !m.is_finite()) {
        tracing::warn!("Invalid camera WB multipliers: {cam_mul:?}, skipping WB");
        return None;
    }

    // Normalize so minimum is 1.0
    let min_mul = mul.iter().copied().fold(f32::MAX, f32::min);
    for m in &mut mul {
        *m /= min_mul;
    }

    tracing::debug!("WB multipliers (normalized): {mul:?} (raw: {cam_mul:?})");
    Some(mul)
}

/// Apply per-channel black delta correction and white balance to Bayer data.
///
/// Operates on data already normalized with the common black level.
/// Channel is determined by the libraw `filters` bitmask using the FC macro.
fn apply_channel_corrections(
    data: &mut [f32],
    raw_width: usize,
    filters: u32,
    delta_norm: &[f32; 4],
    wb_mul: &[f32; 4],
) {
    // Check if corrections are trivial (skip for performance)
    let has_delta = delta_norm.iter().any(|&d| d.abs() > f32::EPSILON);
    let has_wb = wb_mul.iter().any(|&w| (w - 1.0).abs() > f32::EPSILON);
    if !has_delta && !has_wb {
        return;
    }

    data.par_chunks_mut(raw_width)
        .enumerate()
        .for_each(|(row, row_data)| {
            for (col, pixel) in row_data.iter_mut().enumerate() {
                let ch = fc(filters, row, col);
                *pixel = ((*pixel - delta_norm[ch]).max(0.0) * wb_mul[ch]).min(1.0);
            }
        });
}

/// Libraw FC macro: determine color channel at (row, col) from filters bitmask.
#[inline(always)]
fn fc(filters: u32, row: usize, col: usize) -> usize {
    ((filters >> (((row << 1 & 14) | (col & 1)) << 1)) & 3) as usize
}

fn raw_err(path: &Path, reason: impl Into<String>) -> ImageError {
    ImageError::Raw {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
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
    filters: u32,
    wb_multipliers: Option<[f32; 4]>,
    sensor_type: SensorType,
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
    /// Applies per-channel black correction but NO white balance (used for
    /// calibration frames where raw data integrity is required).
    fn extract_cfa_pixels(&self) -> Result<Vec<f32>, ImageError> {
        let raw_data = self.raw_image_slice()?;

        // Pass 1: SIMD normalize with common black level
        let mut normalized = normalize_u16_to_f32_parallel(
            raw_data,
            self.black_level.common,
            self.black_level.inv_range,
        );

        // Pass 2: Per-channel black delta correction (no WB)
        let has_delta = self
            .black_level
            .channel_delta_norm
            .iter()
            .any(|&d| d.abs() > f32::EPSILON);
        if has_delta {
            apply_channel_corrections(
                &mut normalized,
                self.raw_width,
                self.filters,
                &self.black_level.channel_delta_norm,
                &[1.0; 4], // No WB for calibration path
            );
        }

        // Extract the active area
        let output_size = self.width * self.height;
        // SAFETY: Every element is written by the parallel copy_from_slice pass below.
        let mut pixels = unsafe { alloc_uninit_vec::<f32>(output_size) };
        let width = self.width;
        let raw_width = self.raw_width;
        let top_margin = self.top_margin;
        let left_margin = self.left_margin;
        pixels
            .par_chunks_mut(width)
            .enumerate()
            .for_each(|(y, row)| {
                let src_y = top_margin + y;
                let src_start = src_y * raw_width + left_margin;
                row.copy_from_slice(&normalized[src_start..src_start + width]);
            });

        Ok(pixels)
    }

    /// Process Bayer sensor data using our fast SIMD demosaic.
    fn demosaic_bayer(&self, cfa_pattern: CfaPattern) -> Result<Vec<f32>, ImageError> {
        let raw_data = self.raw_image_slice()?;

        // Pass 1: SIMD normalize with common black level
        let mut normalized_data = normalize_u16_to_f32_parallel(
            raw_data,
            self.black_level.common,
            self.black_level.inv_range,
        );

        // Pass 2: Per-channel black delta + white balance
        let wb = self.wb_multipliers.unwrap_or([1.0; 4]);
        apply_channel_corrections(
            &mut normalized_data,
            self.raw_width,
            self.filters,
            &self.black_level.channel_delta_norm,
            &wb,
        );

        let bayer = BayerImage::with_margins(
            &normalized_data,
            self.raw_width,
            self.raw_height,
            self.width,
            self.height,
            self.top_margin,
            self.left_margin,
            cfa_pattern,
        );

        let demosaic_start = Instant::now();
        let rgb_pixels = demosaic_bayer(&bayer);
        let demosaic_elapsed = demosaic_start.elapsed();

        tracing::info!(
            "Fast SIMD demosaicing {}x{} took {:.2}ms",
            self.width,
            self.height,
            demosaic_elapsed.as_secs_f64() * 1000.0
        );

        Ok(rgb_pixels)
    }

    /// Extract X-Trans 6x6 pattern from libraw metadata.
    fn xtrans_pattern(&self) -> [[u8; 6]; 6] {
        // SAFETY: inner is valid and xtrans is populated for X-Trans sensors
        let xtrans_raw = unsafe { (*self.inner).idata.xtrans };
        let mut pattern = [[0u8; 6]; 6];
        for (i, row) in xtrans_raw.iter().enumerate() {
            for (j, &val) in row.iter().enumerate() {
                #[allow(clippy::unnecessary_cast)]
                {
                    pattern[i][j] = val as u8;
                }
            }
        }
        pattern
    }

    /// Process X-Trans sensor data using our Markesteijn demosaic.
    ///
    /// Drops guard and buf before the expensive demosaicing step,
    /// reducing peak memory by ~77 MB.
    fn demosaic_xtrans(&mut self) -> Result<Vec<f32>, ImageError> {
        let raw_data = self.raw_image_slice()?;
        let xtrans_pattern = self.xtrans_pattern();

        // Copy raw u16 data so we can drop libraw before demosaicing.
        // P×2 bytes (~47 MB) instead of P×4 bytes (~93 MB) for normalized f32.
        let raw_u16: Vec<u16> = raw_data.to_vec();

        // Convert 4-channel black/WB to 3-channel for X-Trans (R=0, G=1, B=2)
        let bl = &self.black_level;
        let channel_black = [bl.per_channel[0], bl.per_channel[1], bl.per_channel[2]];
        let wb_mul = match self.wb_multipliers {
            Some(wb) => [wb[0], wb[1], wb[2]],
            None => [1.0; 3],
        };

        // Drop libraw and file buffer to reduce peak memory during demosaicing
        self.guard.take();
        self.buf.take();

        let pixels = process_xtrans(
            &raw_u16,
            self.raw_width,
            self.raw_height,
            self.width,
            self.height,
            self.top_margin,
            self.left_margin,
            xtrans_pattern,
            channel_black,
            bl.inv_range,
            wb_mul,
        );

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
            // Use camera white balance
            (*self.inner).params.use_camera_wb = 1;
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
/// Performs: file read, libraw init, open_buffer, unpack, dimension/color
/// validation, sensor type detection, and ISO extraction.
fn open_raw(path: &Path) -> Result<UnpackedRaw, ImageError> {
    let buf = fs::read(path).map_err(|e| ImageError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;

    // SAFETY: libraw_init returns a valid pointer or null on failure.
    let inner = unsafe { sys::libraw_init(0) };
    if inner.is_null() {
        return Err(raw_err(path, "libraw: Failed to initialize"));
    }

    // Guard ensures cleanup even on early return or panic
    let guard = LibrawGuard(inner);

    // SAFETY: inner is valid (checked above), buf is valid for the duration of this call.
    let ret = unsafe { sys::libraw_open_buffer(inner, buf.as_ptr() as *const _, buf.len()) };
    if ret != 0 {
        return Err(raw_err(
            path,
            format!("libraw: Failed to open buffer, error code: {}", ret),
        ));
    }

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
    let filters = unsafe { (*inner).idata.filters };
    let colors = unsafe { (*inner).idata.colors };
    let sensor_type = detect_sensor_type(filters, colors);

    tracing::debug!(
        "libraw: filters=0x{:08x}, colors={}, sensor_type={:?}",
        filters,
        colors,
        sensor_type
    );

    // Consolidate per-channel black levels (replicates libraw adjust_bl)
    // SAFETY: inner is valid, color.cblack is initialized after unpack.
    let cblack_raw: [u32; 4104] = unsafe { (*inner).color.cblack };
    let black_level = consolidate_black_levels(&cblack_raw, black_raw, maximum_raw, filters);

    // Extract camera white balance multipliers
    // SAFETY: inner is valid, color.cam_mul is initialized after unpack.
    let cam_mul: [f32; 4] = unsafe { (*inner).color.cam_mul };
    let wb_multipliers = compute_wb_multipliers(cam_mul);

    let iso = extract_iso(inner);

    Ok(UnpackedRaw {
        inner,
        guard: Some(guard),
        buf: Some(buf),
        path: path.to_path_buf(),
        raw_width,
        raw_height,
        width,
        height,
        top_margin,
        left_margin,
        black_level,
        filters,
        wb_multipliers,
        sensor_type,
        iso,
    })
}

/// Load raw file using libraw (C library, broader camera support).
///
/// Demosaicing strategy:
/// - Monochrome sensors: no demosaic needed, returns grayscale
/// - Known Bayer patterns (RGGB, BGGR, GRBG, GBRG): fast SIMD demosaic
/// - Unknown patterns (X-Trans, etc.): libraw's built-in demosaic (slower but correct)
pub fn load_raw(path: &Path) -> Result<AstroImage, ImageError> {
    let mut raw = open_raw(path)?;

    let sensor_type = raw.sensor_type.clone();
    let (pixels, width, height, channels, cfa_type) = match sensor_type {
        SensorType::Monochrome => {
            tracing::info!("Monochrome sensor detected, skipping demosaic");
            let pixels = raw.extract_cfa_pixels()?;
            (pixels, raw.width, raw.height, 1, None)
        }
        SensorType::Bayer(cfa_pattern) => {
            tracing::debug!("Detected Bayer CFA pattern: {:?}", cfa_pattern);
            let pixels = raw.demosaic_bayer(cfa_pattern)?;
            (
                pixels,
                raw.width,
                raw.height,
                3,
                Some(CfaType::Bayer(cfa_pattern)),
            )
        }
        SensorType::XTrans => {
            tracing::info!("X-Trans sensor detected, using X-Trans demosaic");
            let xtrans_pattern = raw.xtrans_pattern();
            let pixels = raw.demosaic_xtrans()?;
            (
                pixels,
                raw.width,
                raw.height,
                3,
                Some(CfaType::XTrans(xtrans_pattern)),
            )
        }
        SensorType::Unknown => {
            tracing::info!("Unknown CFA pattern, using libraw demosaic fallback");
            let (pixels, w, h, c) = raw.demosaic_libraw_fallback()?;
            (pixels, w, h, c, Some(CfaType::Mono))
        }
    };

    let metadata = AstroImageMetadata {
        iso: raw.iso,
        bitpix: BitPix::UInt16,
        header_dimensions: vec![height, width, channels],
        cfa_type,
        ..Default::default()
    };
    drop(raw);

    let dimensions = ImageDimensions::new(width, height, channels);
    assert!(
        pixels.len() == dimensions.sample_count(),
        "Sample count mismatch: expected {}, got {}",
        dimensions.sample_count(),
        pixels.len()
    );

    let mut astro = AstroImage::from_pixels(dimensions, pixels);
    astro.metadata = metadata;
    Ok(astro)
}

/// Load raw file and return un-demosaiced CFA data.
///
/// Returns single-channel f32 data with CFA pattern metadata.
/// Used for calibration frame processing (darks, flats, bias)
/// where hot pixel correction must happen before demosaicing.
///
/// For Unknown sensor types, falls back to `load_raw()` then wraps
/// the demosaiced result as a Mono CfaImage.
pub fn load_raw_cfa(path: &Path) -> Result<CfaImage, ImageError> {
    let raw = open_raw(path)?;

    let cfa_type = match &raw.sensor_type {
        SensorType::Monochrome => CfaType::Mono,
        SensorType::Bayer(p) => CfaType::Bayer(*p),
        SensorType::XTrans => CfaType::XTrans(raw.xtrans_pattern()),
        SensorType::Unknown => {
            unimplemented!("Cannot extract raw CFA data for unknown sensor types")
        }
    };

    let pixels = raw.extract_cfa_pixels()?;
    let metadata = AstroImageMetadata {
        iso: raw.iso,
        bitpix: BitPix::UInt16,
        header_dimensions: vec![raw.height, raw.width, 1],
        cfa_type: Some(cfa_type),
        ..Default::default()
    };

    Ok(CfaImage {
        data: Buffer2::new(raw.width, raw.height, pixels),
        metadata,
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
