//! Benchmark utilities for lumos.

use crate::{astro_image::demosaic::CfaPattern, testing::first_raw_file};

/// Returns the raw Bayer data from the first light image for benchmarking.
/// Returns: (data, raw_width, raw_height, width, height, top_margin, left_margin, cfa_pattern)
///
/// # Panics
/// Panics if LUMOS_CALIBRATION_DIR is not set or no light images found.
pub fn first_light_raw_bayer() -> (
    Vec<f32>,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    CfaPattern,
) {
    use libraw_sys as sys;
    use std::fs;
    use std::slice;

    let raw_file = first_raw_file().expect("LUMOS_CALIBRATION_DIR must be set with Lights subdir");
    let buf = fs::read(&raw_file).expect("Failed to read raw file");

    // SAFETY: libraw_init returns a valid pointer or null on failure.
    let inner = unsafe { sys::libraw_init(0) };
    assert!(!inner.is_null(), "libraw: Failed to initialize");

    // SAFETY: inner is valid (checked above), buf is valid for the duration of this call.
    let ret = unsafe { sys::libraw_open_buffer(inner, buf.as_ptr() as *const _, buf.len()) };
    assert!(
        ret == 0,
        "libraw: Failed to open buffer, error code: {}",
        ret
    );

    // SAFETY: inner is valid and open_buffer succeeded.
    let ret = unsafe { sys::libraw_unpack(inner) };
    assert!(ret == 0, "libraw: Failed to unpack, error code: {}", ret);

    // SAFETY: inner is valid and unpack succeeded, sizes struct is initialized.
    let raw_width = unsafe { (*inner).sizes.raw_width } as usize;
    let raw_height = unsafe { (*inner).sizes.raw_height } as usize;
    let width = unsafe { (*inner).sizes.width } as usize;
    let height = unsafe { (*inner).sizes.height } as usize;
    let top_margin = unsafe { (*inner).sizes.top_margin } as usize;
    let left_margin = unsafe { (*inner).sizes.left_margin } as usize;

    // SAFETY: inner is valid, color struct is initialized after unpack.
    let black = unsafe { (*inner).color.black } as f32;
    let maximum = unsafe { (*inner).color.maximum } as f32;
    let range = maximum - black;
    assert!(range > 0.0, "libraw: Invalid color range");

    // SAFETY: inner is valid and unpack succeeded.
    let raw_image_ptr = unsafe { (*inner).rawdata.raw_image };
    assert!(!raw_image_ptr.is_null(), "libraw: raw_image is null");

    let pixel_count = raw_width * raw_height;

    // SAFETY: raw_image_ptr is valid (checked above), and we've validated dimensions.
    let raw_data = unsafe { slice::from_raw_parts(raw_image_ptr, pixel_count) };

    // Normalize to 0.0-1.0 range
    let data: Vec<f32> = raw_data
        .iter()
        .map(|&v| ((v as f32) - black).max(0.0) / range)
        .collect();

    // SAFETY: inner is valid
    unsafe { sys::libraw_close(inner) };

    // Assume RGGB pattern (most common for libraw)
    let cfa = CfaPattern::Rggb;

    (
        data,
        raw_width,
        raw_height,
        width,
        height,
        top_margin,
        left_margin,
        cfa,
    )
}
