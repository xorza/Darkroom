use crate::image::{ChannelSize, ColorFormat, Image};

#[test]
fn it_works() {
    let tiff = Image::read_file("../test_resources/rgb-sample-32bit.tiff").unwrap();
    assert_eq!(tiff.width, 256);
    assert_eq!(tiff.height, 1);
    assert_eq!(tiff.stride, 4096);
    assert_eq!(tiff.channel_size, ChannelSize::_32bit);
    assert_eq!(tiff.color_format, ColorFormat::Rgba);

    let png = Image::read_file("../test_resources/rgba-sample-8bit.png").unwrap();
    assert_eq!(png.width, 864);
    assert_eq!(png.height, 409);
    assert_eq!(png.stride, 3456);
    assert_eq!(png.channel_size, ChannelSize::_8bit);
    assert_eq!(png.color_format, ColorFormat::Rgba);

    let png = Image::read_file("../test_resources/rgb-sample-8bit.png").unwrap();
    assert_eq!(png.width, 331);
    assert_eq!(png.height, 126);
    assert_eq!(png.stride, 1324);
    assert_eq!(png.channel_size, ChannelSize::_8bit);
    assert_eq!(png.color_format, ColorFormat::Rgba);

    png.save_file("test.png").unwrap();
}