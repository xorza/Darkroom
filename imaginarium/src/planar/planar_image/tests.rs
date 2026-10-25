use common::{Buffer2, Size2us};

use crate::color::color_format::ColorFormat;
use crate::error::PlanesError;
use crate::interleaved::interleaved_desc::InterleavedDesc;
use crate::interleaved::interleaved_image::InterleavedImage;
use crate::internals::create_test_image;
use crate::planar::channel_layout::ChannelLayout;
use crate::planar::planar_desc::PlanarDesc;
use crate::planar::planar_image::PlanarImage;

fn f32_image(format: ColorFormat, size: Size2us, samples: &[f32]) -> InterleavedImage {
    InterleavedImage::new_with_data(
        InterleavedDesc::new(size, format),
        bytemuck::cast_slice(samples).to_vec(),
    )
    .unwrap()
}

/// Interleaved RGB samples split into row-major planes, and back into the same samples.
#[test]
fn interleaved_samples_split_into_planes_and_back() {
    let samples: Vec<f32> = (1..=12).map(|v| v as f32).collect();
    let image =
        PlanarImage::from_interleaved(PlanarDesc::new((2, 2), ChannelLayout::Rgb), samples.clone());
    assert_eq!(image.channel(0).pixels(), &[1.0, 4.0, 7.0, 10.0]);
    assert_eq!(image.channel(1).pixels(), &[2.0, 5.0, 8.0, 11.0]);
    assert_eq!(image.channel(2).pixels(), &[3.0, 6.0, 9.0, 12.0]);

    let interleaved = InterleavedImage::from(&image);
    assert_eq!(
        interleaved.desc(),
        InterleavedDesc::new(Size2us::new(2, 2), ColorFormat::RGB_F32)
    );
    assert_eq!(
        interleaved.bytes(),
        bytemuck::cast_slice::<f32, u8>(&samples)
    );

    let mono = PlanarImage::from_interleaved(
        PlanarDesc::new((3, 2), ChannelLayout::Mono),
        vec![0.0, 0.25, 0.5, 0.75, 1.0, 0.5],
    );
    let interleaved = InterleavedImage::from(&mono);
    assert_eq!(interleaved.desc().color_format(), ColorFormat::MONO_F32);
    assert_eq!(
        interleaved.bytes(),
        bytemuck::cast_slice::<f32, u8>(&[0.0, 0.25, 0.5, 0.75, 1.0, 0.5])
    );
}

/// Each plane is the allocation it came in, from samples or from a buffer.
#[test]
fn planes_keep_their_allocations() {
    let plane = vec![1.0, 2.0, 3.0, 4.0];
    let pointer = plane.as_ptr();
    let image =
        PlanarImage::from_planar_channels(PlanarDesc::new((2, 2), ChannelLayout::Mono), [plane]);
    assert!(image.is_mono());
    assert_eq!(image.desc(), PlanarDesc::new((2, 2), ChannelLayout::Mono));
    assert_eq!(image.channel(0).pixels().as_ptr(), pointer);

    let planes = [
        Buffer2::new((2, 1), vec![7.0, 8.0]),
        Buffer2::new((2, 1), vec![9.0, 10.0]),
        Buffer2::new((2, 1), vec![11.0, 12.0]),
    ];
    let green = planes[1].pixels().as_ptr();
    let image = PlanarImage::from(planes);
    assert!(image.is_rgb());
    assert_eq!(image.desc(), PlanarDesc::new((2, 1), ChannelLayout::Rgb));
    assert_eq!(image.channel(1).pixels(), &[9.0, 10.0]);
    assert_eq!(image.channel(1).pixels().as_ptr(), green);
    assert_eq!(image.into_rgb()[1].pixels().as_ptr(), green);
}

#[test]
fn a_channel_written_is_its_plane() {
    let mut image = PlanarImage::new_zeroed(PlanarDesc::new((2, 2), ChannelLayout::Rgb));
    image.channel_mut(2)[3] = 40.0;
    image.rgb_planes_mut()[0][1] = 10.0;
    assert_eq!(image.channel(0).pixels(), &[0.0, 10.0, 0.0, 0.0]);
    assert_eq!(image.channel(1).pixels(), &[0.0; 4]);
    assert_eq!(image.channel(2).pixels(), &[0.0, 0.0, 0.0, 40.0]);
}

#[test]
#[should_panic(expected = "all RGB planes must share a size")]
fn rgb_planes_of_two_sizes_are_refused() {
    let _image = PlanarImage::from([
        Buffer2::new((2, 1), vec![1.0, 2.0]),
        Buffer2::new((1, 1), vec![3.0]),
        Buffer2::new((2, 1), vec![4.0, 5.0]),
    ]);
}

#[test]
#[should_panic(expected = "width must be positive")]
fn an_empty_plane_is_refused() {
    let _image = PlanarImage::from(Buffer2::<f32>::new((0, 4), vec![]));
}

#[test]
#[should_panic(expected = "2x1x3 holds fewer planes")]
fn a_fourth_plane_is_refused() {
    let _image = PlanarImage::from_planar_channels(
        PlanarDesc::new((2, 1), ChannelLayout::Rgb),
        vec![vec![0.0; 2]; 4],
    );
}

/// Every format planes hold goes into `f32` planes and back into its own format byte for byte,
/// borrowed and taken: `u8` and `u16` values survive `f32` exactly. A format with alpha is
/// refused.
#[test]
fn each_format_round_trips_through_the_planes_or_is_refused() {
    for format in ColorFormat::ALL {
        let image = create_test_image(format, Size2us::new(5, 3), 7);
        if format.has_alpha() {
            for result in [
                PlanarImage::try_from(&image),
                PlanarImage::try_from(image.clone()),
            ] {
                assert_eq!(
                    result.unwrap_err(),
                    PlanesError::Alpha {
                        color_format: format
                    },
                    "{format}"
                );
            }
            continue;
        }
        let borrowed = PlanarImage::try_from(&image).unwrap();
        let taken = PlanarImage::try_from(image.clone()).unwrap();
        for planes in [borrowed, taken] {
            assert_eq!(
                planes.channel_count(),
                format.channel_count.count(),
                "{format}"
            );
            let back = InterleavedImage::from(&planes).convert(format);
            assert_eq!(back.desc(), image.desc(), "{format}");
            assert_eq!(back.bytes(), image.bytes(), "{format}");
        }
    }
}

/// A `MONO_F32` image taken hands its allocation to its plane.
#[test]
fn a_mono_f32_image_taken_keeps_its_allocation() {
    let image = f32_image(ColorFormat::MONO_F32, Size2us::new(3, 1), &[0.5, 1.0, 2.0]);
    let pointer = image.bytes().as_ptr();
    let planes = PlanarImage::try_from(image).unwrap();
    assert_eq!(planes.channel(0).pixels(), &[0.5, 1.0, 2.0]);
    assert_eq!(planes.into_mono().pixels().as_ptr().cast(), pointer);
}

/// Dropping alpha keeps each color channel exactly, in the sample type it had: RGBA becomes RGB,
/// and an image with no alpha is itself.
#[test]
fn without_alpha_keeps_the_color_channels() {
    let rgba = f32_image(
        ColorFormat::RGBA_F32,
        Size2us::new(2, 1),
        &[1.0, 0.25, 0.0, 0.5, 0.0, 1.0, 0.75, 1.0],
    );
    let rgb = rgba.without_alpha();
    assert_eq!(rgb.desc().color_format(), ColorFormat::RGB_F32);
    assert_eq!(
        rgb.bytes(),
        bytemuck::cast_slice::<f32, u8>(&[1.0, 0.25, 0.0, 0.0, 1.0, 0.75])
    );

    for format in ColorFormat::ALL {
        let image = create_test_image(format, Size2us::new(4, 2), 1);
        let dropped = image.clone().without_alpha();
        assert_eq!(
            dropped.desc().color_format(),
            format.without_alpha(),
            "{format}"
        );
        let sample = format.sample_type.byte_len();
        let kept = format.without_alpha().channel_count.count() * sample;
        let expected: Vec<u8> = image
            .bytes()
            .chunks_exact(format.byte_len())
            .flat_map(|pixel| &pixel[..kept])
            .copied()
            .collect();
        assert_eq!(dropped.bytes(), expected, "{format}");
    }
}
