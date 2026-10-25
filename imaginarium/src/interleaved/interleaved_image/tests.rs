use common::{Buffer2, Size2us, par};
use rayon::iter::ParallelIterator;

use crate::color::color_format::{ColorFormat, SampleType};
use crate::error::{ByteLenError, PlanesError};
use crate::interleaved::interleaved_desc::InterleavedDesc;
use crate::interleaved::interleaved_image::InterleavedImage;
use crate::internals::create_test_image;

/// One `MONO_U16` pixel is 2 bytes: 3 are refused with both lengths, and 2 make the image.
#[test]
fn data_of_another_length_is_refused() {
    let desc = InterleavedDesc::new(Size2us::new(1, 1), ColorFormat::MONO_U16);
    let error = InterleavedImage::new_with_data(desc, vec![0; 3]).unwrap_err();
    assert_eq!(error, ByteLenError { desc, byte_len: 3 });
    assert_eq!(
        error.to_string(),
        "3 bytes are no 1x1 Mono u16 image, which holds 2"
    );
    let image = InterleavedImage::new_with_data(desc, vec![1, 0]).unwrap();
    assert_eq!(image.bytes(), &[1, 0]);
}

/// A `u8` format keeps the caller's allocation both ways; `u16` and `f32` copy into storage
/// aligned for their type, which a `Vec<u8>` does not promise.
#[test]
fn byte_vectors_are_reused_where_the_layout_allows() {
    for format in ColorFormat::ALL {
        let desc = InterleavedDesc::new(Size2us::new(6, 2), format);
        let bytes: Vec<u8> = (0..desc.byte_len()).map(|i| (i * 7 % 251) as u8).collect();
        let given = bytes.clone();
        let pointer = given.as_ptr();
        let image = InterleavedImage::new_with_data(desc, given).unwrap();
        assert_eq!(image.bytes(), bytes, "{format}");
        assert_eq!(
            image.bytes().as_ptr() as usize % format.sample_type.byte_len(),
            0,
            "{format}: storage is aligned for its sample type"
        );
        assert_eq!(
            image.bytes().as_ptr() == pointer,
            format.sample_type.byte_len() == 1,
            "{format}"
        );

        let pointer = image.bytes().as_ptr();
        let back = image.into_bytes();
        assert_eq!(back, bytes, "{format}");
        assert_eq!(
            back.as_ptr() == pointer,
            format.sample_type.byte_len() == 1,
            "{format}"
        );
    }
}

/// The same format is a move-through: the very same allocation comes back.
#[test]
fn convert_to_the_same_format_keeps_the_allocation() {
    let image = create_test_image(ColorFormat::RGBA_U16, Size2us::new(3, 2), 0);
    let pointer = image.bytes().as_ptr();
    assert_eq!(
        image.convert(ColorFormat::RGBA_U16).bytes().as_ptr(),
        pointer
    );
}

/// RGB 2×1: planes R = [1, 4], G = [2, 5], B = [3, 6] ⟷ interleaved [1, 2, 3, 4, 5, 6].
#[test]
fn planes_interleave_into_the_format_they_spell() {
    let planes = [
        Buffer2::new((2, 1), vec![1u8, 4]),
        Buffer2::new((2, 1), vec![2u8, 5]),
        Buffer2::new((2, 1), vec![3u8, 6]),
    ];
    let image = InterleavedImage::from(planes.each_ref());
    assert_eq!(
        image.desc(),
        InterleavedDesc::new(Size2us::new(2, 1), ColorFormat::RGB_U8)
    );
    assert_eq!(image.bytes(), &[1, 2, 3, 4, 5, 6]);

    let back: [Buffer2<u8>; 3] = (&image).try_into().unwrap();
    assert_eq!(back, planes);
    let taken: [Buffer2<u8>; 3] = image.try_into().unwrap();
    assert_eq!(taken, planes);

    let mono = InterleavedImage::from([&Buffer2::new((3, 1), vec![10u16, 20, 30])]);
    assert_eq!(mono.desc().color_format(), ColorFormat::MONO_U16);
    assert_eq!(mono.bytes(), bytemuck::cast_slice::<u16, u8>(&[10, 20, 30]));
    // A mono image taken is its one plane, in the same allocation.
    let pointer = mono.bytes().as_ptr();
    let [plane]: [Buffer2<u16>; 1] = mono.try_into().unwrap();
    assert_eq!(plane.pixels(), &[10, 20, 30]);
    assert_eq!(plane.pixels().as_ptr().cast::<u8>(), pointer);
}

/// Interleaving planes is parallel work, so a parallel loop's body that converts them panics, as
/// `common::par` refuses nested work; at top level the same conversion returns the image.
#[test]
#[should_panic(expected = "a parallel loop's body starts no parallel work")]
fn planes_converted_in_a_parallel_body_panic() {
    let plane = Buffer2::new((2, 1), vec![1.0f32, 2.0]);
    let convert = || {
        let image = InterleavedImage::from([&plane, &plane, &plane]);
        assert_eq!(
            image.desc(),
            InterleavedDesc::new(Size2us::new(2, 1), ColorFormat::RGB_F32)
        );
    };
    convert();
    par::into_iter(0..2).for_each(|_| convert());
}

#[test]
fn deinterleaving_into_another_format_is_refused() {
    let image = InterleavedImage::new_black(InterleavedDesc::new(
        Size2us::new(2, 2),
        ColorFormat::RGB_U8,
    ));
    let refused = PlanesError::Mismatch {
        color_format: ColorFormat::RGB_U8,
        plane_count: 1,
        sample_type: SampleType::F32,
    };
    let result: Result<[Buffer2<f32>; 1], PlanesError> = (&image).try_into();
    assert_eq!(result.unwrap_err(), refused);
    let result: Result<[Buffer2<f32>; 1], PlanesError> = image.try_into();
    assert_eq!(result.unwrap_err(), refused);
    assert_eq!(
        refused.to_string(),
        "cannot deinterleave a RGB u8 image into 1 f32 planes"
    );
}

#[test]
#[should_panic(expected = "an image needs at least one pixel")]
fn empty_planes_are_refused() {
    let empty: Buffer2<f32> = Buffer2::new((0, 3), Vec::new());
    drop(InterleavedImage::from([&empty]));
}
