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

    png.save_file("test.png").unwrap();
}

#[test]
fn image_convertion() {
    let mut png = Image::read_file("../test_resources/rgb-sample-8bit.png").unwrap();

    let png = png.convert(ChannelCount::Gray, ChannelSize::_16bit, ChannelType::UInt)
        .expect("Failed to convert image");

    png.save_file("test.png").unwrap();
}

#[test]
fn test1() {
    let a: i8 = -10i8;
    let b: u8 = a as u8;
    println!("{}", b);

    println!("{}", i8::MAX);
}