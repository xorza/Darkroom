use crate::common::buffer2::Buffer2;
use crate::common::color_format::ColorFormat;
use crate::image::pixels::image_pixels::ImagePixels;
use crate::image::pixels::interleaved::InterleavedPixels;
use crate::image::pixels::planar::PlanarPixels;

#[test]
fn interleaved_construction_preserves_typed_pixels_and_bytes() {
    let bytes = [1u8, 2, 3, 4, 5, 6];
    let pixels = InterleavedPixels::<3, u8>::from_bytes(2, 1, &bytes);

    assert_eq!(pixels.buffer.width(), 2);
    assert_eq!(pixels.buffer.height(), 1);
    assert_eq!(pixels.buffer.pixels(), &[[1, 2, 3], [4, 5, 6]]);
    assert_eq!(pixels.as_bytes(), bytes);
}

#[test]
fn interleaved_mutable_bytes_write_through() {
    let mut pixels = InterleavedPixels::<1, u8>::new_zeroed(3, 1);
    pixels.as_bytes_mut().copy_from_slice(&[7, 8, 9]);
    assert_eq!(pixels.buffer.pixels(), &[[7], [8], [9]]);
}

#[test]
fn interleaved_f32_bytes_round_trip_exactly() {
    let values = [0.25f32, 0.5, 0.75, 1.0];
    let pixels =
        InterleavedPixels::<1, f32>::from_bytes(4, 1, bytemuck::cast_slice::<f32, u8>(&values));
    let round_trip: &[f32] = bytemuck::cast_slice(pixels.as_bytes());
    assert_eq!(round_trip, values);
}

#[test]
#[should_panic(expected = "byte length must equal")]
fn interleaved_rejects_wrong_byte_length() {
    let _ = InterleavedPixels::<3, u8>::from_bytes(2, 1, &[1, 2, 3]);
}

#[test]
fn planar_construction_preserves_independent_planes() {
    let planes = PlanarPixels::from_planes([
        Buffer2::new(2, 2, vec![1u8, 2, 3, 4]),
        Buffer2::new(2, 2, vec![5u8, 6, 7, 8]),
        Buffer2::new(2, 2, vec![9u8, 10, 11, 12]),
    ]);

    assert_eq!(planes.planes[0].pixels(), &[1, 2, 3, 4]);
    assert_eq!(planes.planes[1].pixels(), &[5, 6, 7, 8]);
    assert_eq!(planes.planes[2].pixels(), &[9, 10, 11, 12]);
}

#[test]
#[should_panic(expected = "all planes must share width")]
fn planar_rejects_mismatched_dimensions() {
    let _ = PlanarPixels::from_planes([
        Buffer2::new(2, 2, vec![0u8; 4]),
        Buffer2::new(3, 2, vec![0u8; 6]),
        Buffer2::new(2, 2, vec![0u8; 4]),
    ]);
}

#[test]
fn image_pixels_derive_exact_descriptors_for_all_formats() {
    for format in [
        ColorFormat::L_U8,
        ColorFormat::L_U16,
        ColorFormat::L_F32,
        ColorFormat::RGB_U8,
        ColorFormat::RGB_U16,
        ColorFormat::RGB_F32,
        ColorFormat::RGBA_U8,
        ColorFormat::RGBA_U16,
        ColorFormat::RGBA_F32,
    ] {
        let mut pixels = ImagePixels::new_zeroed(format, 4, 2);
        let desc = pixels.desc();

        assert_eq!(desc.width, 4);
        assert_eq!(desc.height, 2);
        assert_eq!(desc.color_format, format);
        assert_eq!(pixels.bytes().len(), desc.size_in_bytes());
        assert!(pixels.bytes().iter().all(|&byte| byte == 0));

        pixels.bytes_mut()[0] = 17;
        assert_eq!(pixels.bytes()[0], 17);
    }
}
