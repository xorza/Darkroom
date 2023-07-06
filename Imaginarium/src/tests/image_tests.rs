use crate::color_format::*;
use crate::image::Image;

#[test]
fn it_works() {
    let tiff = Image::read_file("../test_resources/rgb-sample-32bit.tiff").unwrap();
    assert_eq!(tiff.desc.width(), 256);
    assert_eq!(tiff.desc.height(), 1);
    assert_eq!(tiff.desc.stride(), 3072);
    assert_eq!(tiff.desc.color_format().channel_size, ChannelSize::_32bit);
    assert_eq!(tiff.desc.color_format().channel_count, ChannelCount::Rgb);

    let png = Image::read_file("../test_resources/rgba-sample-8bit.png").unwrap();
    assert_eq!(png.desc.width(), 864);
    assert_eq!(png.desc.height(), 409);
    assert_eq!(png.desc.stride(), 3456);
    assert_eq!(png.desc.color_format().channel_size, ChannelSize::_8bit);
    assert_eq!(png.desc.color_format().channel_count, ChannelCount::Rgba);

    let png = Image::read_file("../test_resources/rgb-sample-8bit.png").unwrap();
    assert_eq!(png.desc.width(), 331);
    assert_eq!(png.desc.height(), 126);
    assert_eq!(png.desc.stride(), 993);
    assert_eq!(png.desc.color_format().channel_size, ChannelSize::_8bit);
    assert_eq!(png.desc.color_format().channel_count, ChannelCount::Rgb);
}


#[test]
fn save_rgb_png() {
    let png = Image::read_file("../test_resources/rgb-sample-8bit.png").unwrap();

    png.save_file("../test_output/save_rgb.png").unwrap();
}

#[test]
fn image_convertion() {
    let png = Image::read_file("../test_resources/rgba-sample-8bit.png").unwrap();
    png.save_file("../test_output/rgba-sample-8bit.png").unwrap();

    png.convert(ColorFormat::GRAY_U16).unwrap()
        .save_file("../test_output/convertion-gray-u16.png").unwrap();

    png.convert(ColorFormat::RGB_U16).unwrap()
        .save_file("../test_output/convertion-rgb-u16.png").unwrap();


    let tiff = Image::read_file("../test_resources/rgb-sample-32bit.tiff").unwrap();
    tiff.save_file("../test_output/rgb-sample-32bit.tiff").unwrap();

    tiff.convert(ColorFormat::GRAY_U16).unwrap()
        .save_file("../test_output/convertion-gray-u16.tiff").unwrap();

    tiff.convert(ColorFormat::RGBA_U16).unwrap()
        .save_file("../test_output/convertion-rgba-u16.tiff").unwrap();

    tiff.convert(ColorFormat::RGBA_U8).unwrap()
        .save_file("../test_output/convertion-rgba-u8.tiff").unwrap();

    tiff.convert(ColorFormat::GRAY_I8).unwrap()
        .save_file("../test_output/convertion-gray-i8.tiff").unwrap();

    tiff.convert(ColorFormat::RGBA_F64).unwrap()
        .save_file("../test_output/convertion-rgba-f64.tiff").unwrap();

    tiff.convert(ColorFormat::RGBA_U64).unwrap()
        .save_file("../test_output/convertion-rgba-u64.tiff").unwrap();

    tiff.convert(ColorFormat::RGBA_I32).unwrap()
        .save_file("../test_output/convertion-gray-i64.tiff").unwrap();

    tiff.convert(ColorFormat::RGBA_F32).unwrap()
        .save_file("../test_output/convertion-rgba-f32.tiff").unwrap();

    tiff.convert(ColorFormat::GRAY_ALPHA_U8).unwrap()
        .save_file("../test_output/convertion-ga-u8.tiff").unwrap();

    tiff.convert(ColorFormat::RGB_I32).unwrap()
        .save_file("../test_output/convertion-rgb-i32.tiff").unwrap();

    tiff.convert(ColorFormat::RGBA_F32).unwrap()
        .convert(ColorFormat::RGBA_U16).unwrap()
        .save_file("../test_output/convertion-x2-rgba-u16.tiff").unwrap();
}
