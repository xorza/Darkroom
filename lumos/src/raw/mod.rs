pub mod demosaic;
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
struct ProcessedImageGuard(*mut sys::libraw_processed_image_t);

impl Drop for ProcessedImageGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: We own this pointer and it was allocated by libraw_dcraw_make_mem_image.
            unsafe { sys::libraw_dcraw_clear_mem(self.0) };
        }
    }
}

/// Unpacked raw file data from libraw, ready for sensor-specific processing.
#[derive(Debug)]
struct UnpackedRaw {
    inner: *mut sys::libraw_data_t,
    guard: Option<LibrawGuard>,
    buf: Option<Vec<u8>>,
    raw_width: usize,
    raw_height: usize,
    width: usize,
    height: usize,
    top_margin: usize,
    left_margin: usize,
    black: f32,
    range: f32,
    sensor_type: SensorType,
    iso: Option<u32>,
}

impl UnpackedRaw {
    /// Get the raw u16 image pointer and total pixel count.
    /// Returns the pointer and count, or an error if null.
    fn raw_image_slice(&self) -> Result<&[u16]> {
        // SAFETY: inner is valid and unpack succeeded.
        let raw_image_ptr = unsafe { (*self.inner).rawdata.raw_image };
        if raw_image_ptr.is_null() {
            anyhow::bail!("libraw: raw_image is null");
        }

        let pixel_count = self
            .raw_width
            .checked_mul(self.raw_height)
            .expect("libraw: raw dimensions overflow");

        // SAFETY: raw_image_ptr is valid (checked above), and dimensions were validated in open_raw.
        Ok(unsafe { slice::from_raw_parts(raw_image_ptr, pixel_count) })
    }

    /// Extract raw CFA pixels as normalized f32 (active area only).
    /// Used by Monochrome, Bayer, and XTrans paths.
    fn extract_cfa_pixels(&self) -> Result<Vec<f32>> {
        let raw_data = self.raw_image_slice()?;

        let inv_range = 1.0 / self.range;
        let normalized = normalize_u16_to_f32_parallel(raw_data, self.black, inv_range);

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
    fn demosaic_bayer(&self, cfa_pattern: CfaPattern) -> Result<(Vec<f32>, usize)> {
        let raw_data = self.raw_image_slice()?;

        // Normalize to 0.0-1.0 range using parallel processing
        let inv_range = 1.0 / self.range;
        let normalized_data = normalize_u16_to_f32_parallel(raw_data, self.black, inv_range);

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

        Ok((rgb_pixels, 3))
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
    fn demosaic_xtrans(&mut self) -> Result<(Vec<f32>, usize)> {
        let raw_data = self.raw_image_slice()?;
        let xtrans_pattern = self.xtrans_pattern();

        // Copy raw u16 data so we can drop libraw before demosaicing.
        // P×2 bytes (~47 MB) instead of P×4 bytes (~93 MB) for normalized f32.
        let raw_u16: Vec<u16> = raw_data.to_vec();
        let inv_range = 1.0 / self.range;

        // Drop libraw and file buffer to reduce peak memory during demosaicing
        self.guard.take();
        self.buf.take();

        let (pixels, channels) = process_xtrans(
            &raw_u16,
            self.raw_width,
            self.raw_height,
            self.width,
            self.height,
            self.top_margin,
            self.left_margin,
            xtrans_pattern,
            self.black,
            inv_range,
        );

        Ok((pixels, channels))
    }

    /// Process unknown CFA pattern using libraw's built-in demosaic.
    /// This is slower but handles exotic sensor patterns correctly.
    /// Returns (pixels, width, height, num_channels).
    fn demosaic_libraw_fallback(&self) -> Result<(Vec<f32>, usize, usize, usize)> {
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
            anyhow::bail!("libraw: dcraw_process failed, error code: {}", ret);
        }

        // Get the processed image
        // SAFETY: inner is valid and dcraw_process succeeded
        let mut errc: i32 = 0;
        let processed_ptr = unsafe { sys::libraw_dcraw_make_mem_image(self.inner, &mut errc) };
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
}

/// Open and unpack a raw file using libraw.
///
/// Performs: file read, libraw init, open_buffer, unpack, dimension/color
/// validation, sensor type detection, and ISO extraction.
fn open_raw(path: &Path) -> Result<UnpackedRaw> {
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

    let iso = extract_iso(inner);

    Ok(UnpackedRaw {
        inner,
        guard: Some(guard),
        buf: Some(buf),
        raw_width,
        raw_height,
        width,
        height,
        top_margin,
        left_margin,
        black,
        range,
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
pub fn load_raw(path: &Path) -> Result<AstroImage> {
    let mut raw = open_raw(path)?;

    let sensor_type = raw.sensor_type.clone();
    let (pixels, out_width, out_height, num_channels, cfa_type) = match sensor_type {
        SensorType::Monochrome => {
            tracing::info!("Monochrome sensor detected, skipping demosaic");
            let pixels = raw.extract_cfa_pixels()?;
            (pixels, raw.width, raw.height, 1, None)
        }
        SensorType::Bayer(cfa_pattern) => {
            tracing::debug!("Detected Bayer CFA pattern: {:?}", cfa_pattern);
            let (pixels, channels) = raw.demosaic_bayer(cfa_pattern)?;
            (
                pixels,
                raw.width,
                raw.height,
                channels,
                Some(CfaType::Bayer(cfa_pattern)),
            )
        }
        SensorType::XTrans => {
            tracing::info!("X-Trans sensor detected, using X-Trans demosaic");
            let xtrans_pattern = raw.xtrans_pattern();
            let (pixels, channels) = raw.demosaic_xtrans()?;
            (
                pixels,
                raw.width,
                raw.height,
                channels,
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
        bitpix: BitPix::Int16,
        header_dimensions: vec![out_height, out_width, num_channels],
        cfa_type,
        ..Default::default()
    };
    drop(raw);

    let dimensions = ImageDimensions::new(out_width, out_height, num_channels);
    assert!(
        pixels.len() == dimensions.pixel_count(),
        "Pixel count mismatch: expected {}, got {}",
        dimensions.pixel_count(),
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
pub fn load_raw_cfa(path: &Path) -> Result<CfaImage> {
    let raw = open_raw(path)?;

    let sensor_type = raw.sensor_type.clone();
    match sensor_type {
        SensorType::Monochrome => {
            let pixels = raw.extract_cfa_pixels()?;
            let metadata = AstroImageMetadata {
                iso: raw.iso,
                bitpix: BitPix::Int16,
                header_dimensions: vec![raw.height, raw.width, 1],
                cfa_type: Some(CfaType::Mono),
                ..Default::default()
            };
            Ok(CfaImage {
                data: Buffer2::new(raw.width, raw.height, pixels),
                metadata,
            })
        }
        SensorType::Bayer(cfa_pattern) => {
            let pixels = raw.extract_cfa_pixels()?;
            let metadata = AstroImageMetadata {
                iso: raw.iso,
                bitpix: BitPix::Int16,
                header_dimensions: vec![raw.height, raw.width, 1],
                cfa_type: Some(CfaType::Bayer(cfa_pattern)),
                ..Default::default()
            };
            Ok(CfaImage {
                data: Buffer2::new(raw.width, raw.height, pixels),
                metadata,
            })
        }
        SensorType::XTrans => {
            let xtrans_pattern = raw.xtrans_pattern();
            let pixels = raw.extract_cfa_pixels()?;
            let metadata = AstroImageMetadata {
                iso: raw.iso,
                bitpix: BitPix::Int16,
                header_dimensions: vec![raw.height, raw.width, 1],
                cfa_type: Some(CfaType::XTrans(xtrans_pattern)),
                ..Default::default()
            };
            Ok(CfaImage {
                data: Buffer2::new(raw.width, raw.height, pixels),
                metadata,
            })
        }
        SensorType::Unknown => {
            unimplemented!("Cannot extract raw CFA data for unknown sensor types")
        }
    }
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
