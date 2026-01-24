use crate::common::conversion::Convert;
use crate::common::test_utils::{
    load_lena_rgba_f32_895x551, load_lena_rgba_u8_895x551, test_output_path,
};
use crate::prelude::*;

// =============================================================================
// File reading tests
// =============================================================================

#[test]
fn read_lena_rgba_8bit() {
    let img = load_lena_rgba_u8_895x551();
    assert_eq!(img.desc().width, 895);
    assert_eq!(img.desc().height, 551);
    assert_eq!(img.desc().stride, 3580); // 895 * 4 = 3580
    assert_eq!(img.desc().color_format.channel_size, ChannelSize::_8bit);
    assert_eq!(img.desc().color_format.channel_count, ChannelCount::Rgba);
    assert_eq!(img.desc().color_format.channel_type, ChannelType::UInt);
}

#[test]
fn read_lena_rgb_converted() {
    let img = load_lena_rgba_u8_895x551()
        .convert(ColorFormat::RGB_U8)
        .unwrap();
    assert_eq!(img.desc().width, 895);
    assert_eq!(img.desc().height, 551);
    assert_eq!(img.desc().stride, 2688); // 895 * 3 = 2685, aligned to 4 = 2688
    assert_eq!(img.desc().color_format.channel_size, ChannelSize::_8bit);
    assert_eq!(img.desc().color_format.channel_count, ChannelCount::Rgb);
}

#[test]
fn read_missing_file_returns_error() {
    let result = Image::read_file("/nonexistent/does_not_exist.png");
    assert!(result.is_err());
}

#[test]
fn read_invalid_extension_returns_error() {
    let result = Image::read_file("/nonexistent/file.xyz");
    assert!(matches!(result, Err(Error::InvalidExtension(_))));
}

#[test]
fn read_case_insensitive_extension() {
    // This test verifies that uppercase extensions work
    // We can't easily test this without actual files, but we verify the code path
    let result = Image::read_file("/nonexistent/does_not_exist.PNG");
    // Should fail with IO error (file not found), not InvalidExtension
    assert!(matches!(
        result,
        Err(Error::Io(_)) | Err(Error::Encoding(_))
    ));
}

// =============================================================================
// File saving tests
// =============================================================================

#[test]
fn save_and_reload_png() {
    let original = load_lena_rgba_u8_895x551()
        .convert(ColorFormat::RGB_U8)
        .unwrap();
    original
        .save_file(test_output_path("save_reload.png"))
        .unwrap();

    let reloaded = Image::read_file(test_output_path("save_reload.png")).unwrap();
    assert_eq!(original.desc(), reloaded.desc());
    assert_eq!(original.bytes(), reloaded.bytes());
}

#[test]
fn save_and_reload_tiff() {
    let original = load_lena_rgba_f32_895x551()
        .convert(ColorFormat::RGB_F32)
        .unwrap();
    original
        .save_file(test_output_path("save_reload.tiff"))
        .unwrap();

    let reloaded = Image::read_file(test_output_path("save_reload.tiff")).unwrap();
    assert_eq!(original.desc().width, reloaded.desc().width);
    assert_eq!(original.desc().height, reloaded.desc().height);
    assert_eq!(original.desc().color_format, reloaded.desc().color_format);
}

#[test]
fn save_tiff_with_misaligned_bytes_returns_error() {
    let desc = ImageDesc::new(1, 1, ColorFormat::GRAY_U16);
    // 3 bytes doesn't match expected size for GRAY_U16 (stride * height)
    let result = Image::new_with_data(desc, vec![0u8; 3]);
    assert!(result.is_err());
}

// =============================================================================
// Image creation tests
// =============================================================================

#[test]
fn new_empty_creates_zeroed_image() {
    let desc = ImageDesc::new(10, 10, ColorFormat::RGBA_U8);
    let img = Image::new_black(desc).unwrap();

    assert!(img.bytes().iter().all(|&b| b == 0));
    assert_eq!(img.bytes().len(), img.desc().stride * img.desc().height);
}

#[test]
fn new_with_data_preserves_bytes() {
    let desc = ImageDesc::new(2, 2, ColorFormat::GRAY_U8);
    let data = vec![1, 2, 0, 0, 3, 4, 0, 0]; // 2x2 with 4-byte stride
    let img = Image::new_with_data(desc, data.clone()).unwrap();

    assert_eq!(img.bytes(), &data[..]);
}

#[test]
fn invalid_float_format_returns_error() {
    // 8-bit float is not valid
    let format = ColorFormat {
        channel_count: ChannelCount::Rgba,
        channel_size: ChannelSize::_8bit,
        channel_type: ChannelType::Float,
    };
    let desc = ImageDesc::new(1, 1, format);
    let result = Image::new_black(desc);

    assert!(matches!(result, Err(Error::InvalidColorFormat(_))));
}

#[test]
fn valid_f32_format_succeeds() {
    let desc = ImageDesc::new(1, 1, ColorFormat::RGBA_F32);
    let result = Image::new_black(desc);
    assert!(result.is_ok());
}

// =============================================================================
// ImageDesc tests
// =============================================================================

#[test]
fn image_desc_stride_alignment() {
    // Stride should be 4-byte aligned
    let desc = ImageDesc::new(1, 1, ColorFormat::GRAY_U8);
    assert_eq!(desc.stride % 4, 0);

    let desc = ImageDesc::new(3, 1, ColorFormat::RGB_U8);
    assert_eq!(desc.stride % 4, 0);
    assert!(desc.stride >= 9); // 3 pixels * 3 bytes

    let desc = ImageDesc::new(5, 1, ColorFormat::RGBA_U16);
    assert_eq!(desc.stride % 4, 0);
}

#[test]
fn image_desc_size_calculation() {
    let desc = ImageDesc::new(100, 50, ColorFormat::RGBA_U8);
    assert_eq!(desc.size_in_bytes(), desc.stride * desc.height);
}

#[test]
fn bytes_per_pixel_calculation() {
    let desc = ImageDesc::new(1, 1, ColorFormat::RGBA_U8);
    let img = Image::new_black(desc).unwrap();
    assert_eq!(img.bytes_per_pixel(), 4);

    let desc = ImageDesc::new(1, 1, ColorFormat::RGB_U16);
    let img = Image::new_black(desc).unwrap();
    assert_eq!(img.bytes_per_pixel(), 6);

    let desc = ImageDesc::new(1, 1, ColorFormat::GRAY_F32);
    let img = Image::new_black(desc).unwrap();
    assert_eq!(img.bytes_per_pixel(), 4);
}

// =============================================================================
// Conversion tests
// =============================================================================

#[test]
fn convert_same_format_returns_same_image() {
    let desc = ImageDesc::new(2, 2, ColorFormat::RGBA_U8);
    let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    let img = Image::new_with_data(desc, data.clone()).unwrap();

    let converted = img.convert(ColorFormat::RGBA_U8).unwrap();
    assert_eq!(converted.bytes(), &data[..]);
}

#[test]
fn convert_rgba_u8_to_rgba_u16() {
    let desc = ImageDesc::new(1, 1, ColorFormat::RGBA_U8);
    let src = Image::new_with_data(desc, vec![0, 128, 255, 64]).unwrap();
    let result = src.convert(ColorFormat::RGBA_U16).unwrap();

    assert_eq!(result.desc().color_format, ColorFormat::RGBA_U16);
    let expected_vals: [u16; 4] = [
        0u8.convert(),
        128u8.convert(),
        255u8.convert(),
        64u8.convert(),
    ];
    let expected_bytes: Vec<u8> = bytemuck::cast_slice(&expected_vals).to_vec();
    assert_eq!(result.bytes(), &expected_bytes[..]);
}

#[test]
fn convert_rgb_u8_to_rgb_u16() {
    let desc = ImageDesc::new(1, 1, ColorFormat::RGB_U8);
    // stride is 4 bytes (3 bytes data + 1 padding for 4-byte alignment)
    let src = Image::new_with_data(desc, vec![0, 128, 255, 0]).unwrap();
    let result = src.convert(ColorFormat::RGB_U16).unwrap();

    assert_eq!(result.desc().color_format, ColorFormat::RGB_U16);
    let expected_vals: [u16; 3] = [0u8.convert(), 128u8.convert(), 255u8.convert()];
    let mut expected_bytes: Vec<u8> = bytemuck::cast_slice(&expected_vals).to_vec();
    expected_bytes.extend_from_slice(&[0, 0]); // padding to 8 bytes
    assert_eq!(result.bytes(), &expected_bytes[..]);
}

#[test]
fn convert_gray_u8_to_gray_u16() {
    let desc = ImageDesc::new(1, 1, ColorFormat::GRAY_U8);
    // stride is 4 bytes (1 byte data + 3 padding for 4-byte alignment)
    let src = Image::new_with_data(desc, vec![200, 0, 0, 0]).unwrap();
    let result = src.convert(ColorFormat::GRAY_U16).unwrap();

    assert_eq!(result.desc().color_format, ColorFormat::GRAY_U16);
    let expected_val: u16 = 200u8.convert();
    let mut expected_bytes: Vec<u8> = bytemuck::cast_slice(&[expected_val]).to_vec();
    expected_bytes.extend_from_slice(&[0, 0]); // padding
    assert_eq!(result.bytes(), &expected_bytes[..]);
}

#[test]
fn convert_channel_count_gray_to_rgb() {
    let desc = ImageDesc::new(1, 1, ColorFormat::GRAY_U8);
    let src = Image::new_with_data(desc, vec![128, 0, 0, 0]).unwrap();
    let result = src.convert(ColorFormat::RGB_U8).unwrap();

    assert_eq!(result.desc().color_format, ColorFormat::RGB_U8);
    // Gray value should be replicated to R, G, B
    assert_eq!(result.bytes()[0], 128); // R
    assert_eq!(result.bytes()[1], 128); // G
    assert_eq!(result.bytes()[2], 128); // B
}

#[test]
fn convert_channel_count_rgb_to_gray() {
    let desc = ImageDesc::new(1, 1, ColorFormat::RGB_U8);
    // R=100, G=150, B=200
    // Luminance (Rec.709) = 0.2126*100 + 0.7152*150 + 0.0722*200 = 142.98
    let src = Image::new_with_data(desc, vec![100, 150, 200, 0]).unwrap();
    let result = src.convert(ColorFormat::GRAY_U8).unwrap();

    assert_eq!(result.desc().color_format, ColorFormat::GRAY_U8);
    // Should be luminance-weighted grayscale
    assert_eq!(result.bytes()[0], 142);
}

#[test]
fn convert_rgba_to_rgb_drops_alpha() {
    let desc = ImageDesc::new(1, 1, ColorFormat::RGBA_U8);
    let src = Image::new_with_data(desc, vec![100, 150, 200, 255]).unwrap();
    let result = src.convert(ColorFormat::RGB_U8).unwrap();

    assert_eq!(result.desc().color_format, ColorFormat::RGB_U8);
    assert_eq!(result.bytes()[0], 100); // R
    assert_eq!(result.bytes()[1], 150); // G
    assert_eq!(result.bytes()[2], 200); // B
}

#[test]
fn convert_rgb_to_rgba_adds_max_alpha() {
    let desc = ImageDesc::new(1, 1, ColorFormat::RGB_U8);
    let src = Image::new_with_data(desc, vec![100, 150, 200, 0]).unwrap();
    let result = src.convert(ColorFormat::RGBA_U8).unwrap();

    assert_eq!(result.desc().color_format, ColorFormat::RGBA_U8);
    assert_eq!(result.bytes()[0], 100); // R
    assert_eq!(result.bytes()[1], 150); // G
    assert_eq!(result.bytes()[2], 200); // B
    assert_eq!(result.bytes()[3], 255); // A = max
}

#[test]
fn convert_to_float_normalizes() {
    let desc = ImageDesc::new(1, 1, ColorFormat::GRAY_U8);
    let src = Image::new_with_data(desc, vec![255, 0, 0, 0]).unwrap();
    let result = src.convert(ColorFormat::GRAY_F32).unwrap();

    let float_val: f32 = bytemuck::cast_slice(&result.bytes()[..4])[0];
    assert!(
        (float_val - 1.0).abs() < 0.01,
        "Expected ~1.0, got {}",
        float_val
    );
}

#[test]
fn convert_from_float_denormalizes() {
    let desc = ImageDesc::new(1, 1, ColorFormat::GRAY_F32);
    let float_bytes: [u8; 4] = 1.0f32.to_ne_bytes();
    let data = float_bytes.to_vec();
    let src = Image::new_with_data(desc, data).unwrap();
    let result = src.convert(ColorFormat::GRAY_U8).unwrap();

    assert_eq!(result.bytes()[0], 255);
}

// =============================================================================
// Integration tests (file-based conversions)
// =============================================================================

#[test]
fn convert_and_save_various_formats() {
    let img = load_lena_rgba_u8_895x551();

    // Test various format conversions
    let conversions = [
        (ColorFormat::GRAY_U8, test_output_path("conv-gray-u8.tiff")),
        (
            ColorFormat::GRAY_U16,
            test_output_path("conv-gray-u16.tiff"),
        ),
        (ColorFormat::RGB_U8, test_output_path("conv-rgb-u8.tiff")),
        (ColorFormat::RGB_U16, test_output_path("conv-rgb-u16.tiff")),
        (
            ColorFormat::RGBA_U16,
            test_output_path("conv-rgba-u16.tiff"),
        ),
        (
            ColorFormat::RGBA_F32,
            test_output_path("conv-rgba-f32.tiff"),
        ),
        (
            ColorFormat::GRAY_ALPHA_U8,
            test_output_path("conv-ga-u8.tiff"),
        ),
    ];

    for (format, path) in conversions {
        img.clone()
            .convert(format)
            .unwrap_or_else(|_| panic!("Failed to convert to {:?}", format))
            .save_file(&path)
            .unwrap_or_else(|_| panic!("Failed to save {:?}", path));
    }
}

#[test]
fn double_conversion_preserves_dimensions() {
    let original = load_lena_rgba_u8_895x551();
    let width = original.desc().width;
    let height = original.desc().height;

    let converted = original
        .convert(ColorFormat::RGBA_F32)
        .unwrap()
        .convert(ColorFormat::RGBA_U16)
        .unwrap();

    assert_eq!(converted.desc().width, width);
    assert_eq!(converted.desc().height, height);
}

// =============================================================================
// Edge case tests
// =============================================================================

#[test]
fn single_pixel_image() {
    let desc = ImageDesc::new(1, 1, ColorFormat::RGBA_U8);
    let img = Image::new_black(desc).unwrap();
    assert!(img.desc().stride >= 4);
}

#[test]
fn large_image_dimensions() {
    let desc = ImageDesc::new(4096, 4096, ColorFormat::RGBA_U8);
    // Just verify it calculates correctly without overflow
    assert_eq!(desc.size_in_bytes(), desc.stride * 4096);
}

#[test]
fn clone_image() {
    let desc = ImageDesc::new(2, 2, ColorFormat::RGBA_U8);
    let data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    let img = Image::new_with_data(desc, data.clone()).unwrap();
    let cloned = img.clone();

    assert_eq!(img.desc(), cloned.desc());
    assert_eq!(img.bytes(), cloned.bytes());
}

// =============================================================================
// Alignment tests (AVec)
// =============================================================================

#[test]
fn image_bytes_are_8_byte_aligned() {
    let desc = ImageDesc::new(100, 100, ColorFormat::RGBA_U8);
    let img = Image::new_black(desc).unwrap();
    let ptr = img.bytes().as_ptr() as usize;
    assert_eq!(ptr % 8, 0, "Image bytes should be 8-byte aligned");
}

#[test]
fn into_bytes_is_zero_copy() {
    let desc = ImageDesc::new(100, 100, ColorFormat::RGBA_U8);
    let img = Image::new_black(desc).unwrap();
    let original_ptr = img.bytes().as_ptr();
    let vec = img.into_bytes();
    let vec_ptr = vec.as_ptr();
    assert_eq!(original_ptr, vec_ptr, "into_bytes should be zero-copy");
}

#[test]
fn new_with_data_from_aligned_vec_preserves_pointer() {
    // Create an aligned Vec by going through AVec
    let mut aligned = aligned_vec::AVec::<u8>::with_capacity(8, 16);
    aligned.resize(16, 42);
    let original_ptr = aligned.as_ptr();

    // Convert to Vec (this is zero-copy from AVec to Vec)
    let (ptr, _align, len, capacity) = aligned.into_raw_parts();
    let vec = unsafe { Vec::from_raw_parts(ptr, len, capacity) };

    // Now create Image from the aligned Vec
    let desc = ImageDesc::new(2, 2, ColorFormat::RGBA_U8);
    let img = Image::new_with_data(desc, vec).unwrap();

    // Should preserve the pointer since it was already 8-byte aligned
    assert_eq!(
        img.bytes().as_ptr(),
        original_ptr,
        "Should preserve pointer for aligned input"
    );
}

#[test]
fn cast_to_f32_slice_works() {
    let desc = ImageDesc::new(1, 1, ColorFormat::RGBA_F32);
    let mut img = Image::new_black(desc).unwrap();

    // Write f32 values via bytemuck
    let floats: &mut [f32] = bytemuck::cast_slice_mut(img.bytes_mut());
    floats[0] = 1.0;
    floats[1] = 0.5;
    floats[2] = 0.25;
    floats[3] = 1.0;

    // Read back
    let floats: &[f32] = bytemuck::cast_slice(img.bytes());
    assert_eq!(floats[0], 1.0);
    assert_eq!(floats[1], 0.5);
    assert_eq!(floats[2], 0.25);
    assert_eq!(floats[3], 1.0);
}
