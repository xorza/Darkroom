pub(crate) mod demosaic;
mod normalize;

#[cfg(test)]
mod benches;
#[cfg(test)]
mod tests;

use anyhow::{Context, Result};
use libraw_sys as sys;
use std::fs;
use std::path::Path;
use std::slice;
use std::time::Instant;

use rayon::prelude::*;

use crate::astro_image::sensor::{SensorType, detect_sensor_type};
use crate::astro_image::{AstroImage, AstroImageMetadata, BitPix, ImageDimensions};
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
struct ProcessedImageGuard(*mut sys::libraw_processed_image_t);

impl Drop for ProcessedImageGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: We own this pointer and it was allocated by libraw_dcraw_make_mem_image.
            unsafe { sys::libraw_dcraw_clear_mem(self.0) };
        }
    }
}

/// Load raw file using libraw (C library, broader camera support).
///
/// Demosaicing strategy:
/// - Monochrome sensors: no demosaic needed, returns grayscale
/// - Known Bayer patterns (RGGB, BGGR, GRBG, GBRG): fast SIMD demosaic
/// - Unknown patterns (X-Trans, etc.): libraw's built-in demosaic (slower but correct)
pub fn load_raw(path: &Path) -> Result<AstroImage> {
    let buf =
        fs::read(path).with_context(|| format!("Failed to read raw file: {}", path.display()))?;

    // SAFETY: libraw_init returns a valid pointer or null on failure.
    let inner = unsafe { sys::libraw_init(0) };
    if inner.is_null() {
        anyhow::bail!("libraw: Failed to initialize");
    }

    // Guard ensures cleanup even on early return or panic
    let guard = LibrawGuard(inner);

    // SAFETY: inner is valid (checked above), buf is valid for the duration of this call.
    let ret = unsafe { sys::libraw_open_buffer(inner, buf.as_ptr() as *const _, buf.len()) };
    if ret != 0 {
        anyhow::bail!("libraw: Failed to open buffer, error code: {}", ret);
    }

    // SAFETY: inner is valid and open_buffer succeeded.
    let ret = unsafe { sys::libraw_unpack(inner) };
    if ret != 0 {
        anyhow::bail!("libraw: Failed to unpack, error code: {}", ret);
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
        anyhow::bail!(
            "libraw: Invalid raw dimensions: {}x{}",
            raw_width,
            raw_height
        );
    }
    if width == 0 || height == 0 {
        anyhow::bail!("libraw: Invalid output dimensions: {}x{}", width, height);
    }
    if top_margin + height > raw_height || left_margin + width > raw_width {
        anyhow::bail!(
            "libraw: Margins exceed raw dimensions: margins ({}, {}) + size ({}, {}) > raw ({}, {})",
            top_margin,
            left_margin,
            width,
            height,
            raw_width,
            raw_height
        );
    }

    // SAFETY: inner is valid, color struct is initialized after unpack.
    let black = unsafe { (*inner).color.black } as f32;
    let maximum = unsafe { (*inner).color.maximum } as f32;
    let range = maximum - black;

    // Validate color range
    if range <= 0.0 {
        anyhow::bail!(
            "libraw: Invalid color range: black={}, maximum={}, range={}",
            black,
            maximum,
            range
        );
    }

    tracing::debug!(
        "libraw: black={}, maximum={}, range={}",
        black,
        maximum,
        range
    );

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

    // Extract ISO before the match — all branches need it, and X-Trans
    // consumes guard/buf so it must be read first.
    let iso = extract_iso(inner);

    // Wrap in Option so X-Trans can take ownership to drop before demosaicing.
    let mut guard = Some(guard);
    let mut buf = Some(buf);

    // Process based on sensor type
    // Returns (pixels, width, height, num_channels, is_cfa) - dimensions may differ for libraw fallback
    let (pixels, out_width, out_height, num_channels, is_cfa) = match sensor_type {
        SensorType::Monochrome => {
            tracing::info!(
                "Monochrome sensor detected (filters=0x{:08x}, colors={}), skipping demosaic",
                filters,
                colors
            );
            let (pixels, channels) = process_monochrome(
                inner,
                raw_width,
                raw_height,
                width,
                height,
                top_margin,
                left_margin,
                black,
                range,
            )?;
            (pixels, width, height, channels, false)
        }
        SensorType::Bayer(cfa_pattern) => {
            tracing::debug!(
                "Detected Bayer CFA pattern: {:?} (filters=0x{:08x})",
                cfa_pattern,
                filters
            );
            let (pixels, channels) = process_bayer_fast(
                inner,
                raw_width,
                raw_height,
                width,
                height,
                top_margin,
                left_margin,
                black,
                range,
                cfa_pattern,
            )?;
            (pixels, width, height, channels, true)
        }
        SensorType::XTrans => {
            tracing::info!(
                "X-Trans sensor detected (filters=0x{:08x}), using X-Trans demosaic",
                filters
            );
            let (pixels, channels) = process_xtrans_fast(
                inner,
                guard.take().unwrap(),
                buf.take().unwrap(),
                raw_width,
                raw_height,
                width,
                height,
                top_margin,
                left_margin,
                black,
                range,
            )?;
            (pixels, width, height, channels, true)
        }
        SensorType::Unknown => {
            tracing::info!(
                "Unknown CFA pattern (filters=0x{:08x}), using libraw demosaic fallback",
                filters
            );
            let (pixels, w, h, c) = process_unknown_libraw_fallback(inner)?;
            (pixels, w, h, c, true) // Assume CFA for unknown patterns
        }
    };

    // Drop libraw and file buffer (no-op if X-Trans already consumed them)
    drop(guard);
    drop(buf);

    let dimensions = ImageDimensions::new(out_width, out_height, num_channels);
    assert!(
        pixels.len() == dimensions.pixel_count(),
        "Pixel count mismatch: expected {}, got {}",
        dimensions.pixel_count(),
        pixels.len()
    );

    let metadata = AstroImageMetadata {
        object: None,
        instrument: None,
        telescope: None,
        date_obs: None,
        exposure_time: None,
        iso,
        bitpix: BitPix::Int16,
        header_dimensions: vec![out_height, out_width, num_channels],
        is_cfa,
    };

    let mut astro = AstroImage::from_pixels(dimensions, pixels);
    astro.metadata = metadata;
    Ok(astro)
}

/// Process monochrome sensor data (no demosaicing needed).
#[allow(clippy::too_many_arguments)]
fn process_monochrome(
    inner: *mut sys::libraw_data_t,
    raw_width: usize,
    raw_height: usize,
    width: usize,
    height: usize,
    top_margin: usize,
    left_margin: usize,
    black: f32,
    range: f32,
) -> Result<(Vec<f32>, usize)> {
    // SAFETY: inner is valid and unpack succeeded.
    let raw_image_ptr = unsafe { (*inner).rawdata.raw_image };
    if raw_image_ptr.is_null() {
        anyhow::bail!("libraw: raw_image is null");
    }

    let pixel_count = raw_width
        .checked_mul(raw_height)
        .expect("libraw: raw dimensions overflow");

    // SAFETY: raw_image_ptr is valid (checked above), and we've validated dimensions.
    let raw_data = unsafe { slice::from_raw_parts(raw_image_ptr, pixel_count) };

    // Normalize full raw buffer using SIMD parallel normalization
    let inv_range = 1.0 / range;
    let normalized = normalize_u16_to_f32_parallel(raw_data, black, inv_range);

    // Extract the active area
    let output_size = width * height;
    // SAFETY: Every element is written by the parallel copy_from_slice pass below.
    let mut mono_pixels = unsafe { alloc_uninit_vec::<f32>(output_size) };
    mono_pixels
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            let src_y = top_margin + y;
            let src_start = src_y * raw_width + left_margin;
            row.copy_from_slice(&normalized[src_start..src_start + width]);
        });

    Ok((mono_pixels, 1))
}

/// Process Bayer sensor data using our fast SIMD demosaic.
#[allow(clippy::too_many_arguments)]
fn process_bayer_fast(
    inner: *mut sys::libraw_data_t,
    raw_width: usize,
    raw_height: usize,
    width: usize,
    height: usize,
    top_margin: usize,
    left_margin: usize,
    black: f32,
    range: f32,
    cfa_pattern: CfaPattern,
) -> Result<(Vec<f32>, usize)> {
    // SAFETY: inner is valid and unpack succeeded.
    let raw_image_ptr = unsafe { (*inner).rawdata.raw_image };
    if raw_image_ptr.is_null() {
        anyhow::bail!("libraw: raw_image is null");
    }

    let pixel_count = raw_width
        .checked_mul(raw_height)
        .expect("libraw: raw dimensions overflow");

    // SAFETY: raw_image_ptr is valid (checked above), and we've validated dimensions.
    let raw_data = unsafe { slice::from_raw_parts(raw_image_ptr, pixel_count) };

    // Normalize to 0.0-1.0 range using parallel processing
    let inv_range = 1.0 / range;
    let normalized_data = normalize_u16_to_f32_parallel(raw_data, black, inv_range);

    let bayer = BayerImage::with_margins(
        &normalized_data,
        raw_width,
        raw_height,
        width,
        height,
        top_margin,
        left_margin,
        cfa_pattern,
    );

    let demosaic_start = Instant::now();
    let rgb_pixels = demosaic_bayer(&bayer);
    let demosaic_elapsed = demosaic_start.elapsed();

    tracing::info!(
        "Fast SIMD demosaicing {}x{} took {:.2}ms",
        width,
        height,
        demosaic_elapsed.as_secs_f64() * 1000.0
    );

    Ok((rgb_pixels, 3))
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

/// Process X-Trans sensor data using our Markesteijn demosaic.
///
/// Takes ownership of `guard` and `buf` to drop them before the expensive
/// demosaicing step, reducing peak memory by ~77 MB.
#[allow(clippy::too_many_arguments)]
fn process_xtrans_fast(
    inner: *mut sys::libraw_data_t,
    guard: LibrawGuard,
    buf: Vec<u8>,
    raw_width: usize,
    raw_height: usize,
    width: usize,
    height: usize,
    top_margin: usize,
    left_margin: usize,
    black: f32,
    range: f32,
) -> Result<(Vec<f32>, usize)> {
    // SAFETY: inner is valid and unpack succeeded.
    let raw_image_ptr = unsafe { (*inner).rawdata.raw_image };
    if raw_image_ptr.is_null() {
        anyhow::bail!("libraw: raw_image is null");
    }

    let pixel_count = raw_width
        .checked_mul(raw_height)
        .expect("libraw: raw dimensions overflow");

    // SAFETY: raw_image_ptr is valid (checked above), and we've validated dimensions.
    let raw_data = unsafe { slice::from_raw_parts(raw_image_ptr, pixel_count) };

    // Get X-Trans 6×6 pattern from libraw
    // SAFETY: inner is valid and xtrans is populated for X-Trans sensors
    let xtrans_raw = unsafe { (*inner).idata.xtrans };

    // Convert libraw's pattern to u8 (type varies by platform)
    let mut xtrans_pattern = [[0u8; 6]; 6];
    for (i, row) in xtrans_raw.iter().enumerate() {
        for (j, &val) in row.iter().enumerate() {
            #[allow(clippy::unnecessary_cast)]
            {
                xtrans_pattern[i][j] = val as u8;
            }
        }
    }

    // Copy raw u16 data so we can drop libraw before demosaicing.
    // P×2 bytes (~47 MB) instead of P×4 bytes (~93 MB) for normalized f32.
    let raw_u16: Vec<u16> = raw_data.to_vec();
    let inv_range = 1.0 / range;

    // Drop libraw and file buffer to reduce peak memory during demosaicing
    drop(guard);
    drop(buf);

    let (pixels, channels) = process_xtrans(
        &raw_u16,
        raw_width,
        raw_height,
        width,
        height,
        top_margin,
        left_margin,
        xtrans_pattern,
        black,
        inv_range,
    );

    Ok((pixels, channels))
}

/// Process unknown CFA pattern using libraw's built-in demosaic.
/// This is slower but handles exotic sensor patterns correctly.
/// Returns (pixels, width, height, num_channels).
fn process_unknown_libraw_fallback(
    inner: *mut sys::libraw_data_t,
) -> Result<(Vec<f32>, usize, usize, usize)> {
    let demosaic_start = Instant::now();

    // Configure libraw for linear output (no gamma, no color conversion)
    // SAFETY: inner is valid
    unsafe {
        // Output in linear color space (no gamma curve)
        (*inner).params.gamm[0] = 1.0;
        (*inner).params.gamm[1] = 1.0;
        // No brightness adjustment
        (*inner).params.bright = 1.0;
        // Use camera white balance
        (*inner).params.use_camera_wb = 1;
        // Output 16-bit
        (*inner).params.output_bps = 16;
        // Linear color space (raw)
        (*inner).params.output_color = 0;
        // No auto-brightness
        (*inner).params.no_auto_bright = 1;
    }

    // Run libraw's demosaic
    // SAFETY: inner is valid and configured
    let ret = unsafe { sys::libraw_dcraw_process(inner) };
    if ret != 0 {
        anyhow::bail!("libraw: dcraw_process failed, error code: {}", ret);
    }

    // Get the processed image
    // SAFETY: inner is valid and dcraw_process succeeded
    let mut errc: i32 = 0;
    let processed_ptr = unsafe { sys::libraw_dcraw_make_mem_image(inner, &mut errc) };
    if processed_ptr.is_null() || errc != 0 {
        anyhow::bail!("libraw: dcraw_make_mem_image failed, error code: {}", errc);
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
