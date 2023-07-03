use crate::image::{ChannelCount, ChannelSize, ChannelType, Image};

#[test]
fn it_works() {
    let tiff = Image::read_file("../test_resources/rgb-sample-32bit.tiff").unwrap();
    assert_eq!(tiff.width, 256);
    assert_eq!(tiff.height, 1);
    assert_eq!(tiff.stride, 3072);
    assert_eq!(tiff.channel_size, ChannelSize::_32bit);
    assert_eq!(tiff.channel_count, ChannelCount::Rgb);

    let png = Image::read_file("../test_resources/rgba-sample-8bit.png").unwrap();
    assert_eq!(png.width, 864);
    assert_eq!(png.height, 409);
    assert_eq!(png.stride, 3456);
    assert_eq!(png.channel_size, ChannelSize::_8bit);
    assert_eq!(png.channel_count, ChannelCount::Rgba);

    let png = Image::read_file("../test_resources/rgb-sample-8bit.png").unwrap();
    assert_eq!(png.width, 331);
    assert_eq!(png.height, 126);
    assert_eq!(png.stride, 993);
    assert_eq!(png.channel_size, ChannelSize::_8bit);
    assert_eq!(png.channel_count, ChannelCount::Rgb);
}


#[test]
fn save_rgb_png() {
    let png = Image::read_file("../test_resources/rgb-sample-8bit.png").unwrap();

    png.save_file("../test_output/save_rgb_png.png").unwrap();
}

#[test]
fn image_convertion() {
    let png = Image::read_file("../test_resources/rgba-sample-8bit.png").unwrap();
    png.save_file("../test_output/rgba-sample-8bit.png").unwrap();

    png.convert(ChannelCount::Gray, ChannelSize::_16bit, ChannelType::UInt).unwrap()
        .save_file("../test_output/convertion-gray-u16.png").unwrap();

    png.convert(ChannelCount::Rgb, ChannelSize::_16bit, ChannelType::UInt).unwrap()
        .save_file("../test_output/convertion-rgb-u16.png").unwrap();


    let tiff = Image::read_file("../test_resources/rgb-sample-32bit.tiff").unwrap();
    tiff.save_file("../test_output/rgb-sample-32bit.tiff").unwrap();

    tiff.convert(ChannelCount::Gray, ChannelSize::_16bit, ChannelType::UInt).unwrap()
        .save_file("../test_output/convertion-gray-u16.tiff").unwrap();

    tiff.convert(ChannelCount::Rgba, ChannelSize::_16bit, ChannelType::UInt).unwrap()
        .save_file("../test_output/convertion-rgba-u16.tiff").unwrap();

    tiff.convert(ChannelCount::Rgba, ChannelSize::_8bit, ChannelType::UInt).unwrap()
        .save_file("../test_output/convertion-rgba-u8.tiff").unwrap();

    tiff.convert(ChannelCount::Gray, ChannelSize::_8bit, ChannelType::Int).unwrap()
        .save_file("../test_output/convertion-gray-i8.tiff").unwrap();

    tiff.convert(ChannelCount::Rgba, ChannelSize::_64bit, ChannelType::Float).unwrap()
        .save_file("../test_output/convertion-rgba-f64.tiff").unwrap();

    tiff.convert(ChannelCount::Rgba, ChannelSize::_64bit, ChannelType::UInt).unwrap()
        .save_file("../test_output/convertion-rgba-u64.tiff").unwrap();

    tiff.convert(ChannelCount::Gray, ChannelSize::_32bit, ChannelType::Int).unwrap()
        .save_file("../test_output/convertion-gray-i64.tiff").unwrap();

    tiff.convert(ChannelCount::Rgba, ChannelSize::_32bit, ChannelType::Float).unwrap()
        .save_file("../test_output/convertion-rgba-f32.tiff").unwrap();

    tiff.convert(ChannelCount::GrayAlpha, ChannelSize::_8bit, ChannelType::UInt).unwrap()
        .save_file("../test_output/convertion-ga-u8.tiff").unwrap();

    tiff.convert(ChannelCount::Rgb, ChannelSize::_32bit, ChannelType::Int).unwrap()
        .save_file("../test_output/convertion-rgb-i32.tiff").unwrap();

    tiff.convert(ChannelCount::Rgba, ChannelSize::_32bit, ChannelType::Float).unwrap()
        .convert(ChannelCount::Rgba, ChannelSize::_16bit, ChannelType::UInt).unwrap()
        .save_file("../test_output/convertion-x2-rgba-u16.tiff").unwrap();
}
