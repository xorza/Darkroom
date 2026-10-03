use imaginarium::{ALL_FORMATS, ColorFormat, Image as CpuImage, ImageDesc};
use lumos::{ImageDimensions, LinearImage};
use scenarium::{CustomValueCodec, Library};

use crate::image::codec::{HEADER_LEN, ImageCodec, image_type_entry};
use crate::image::{IMAGE_TYPE_ID, Image, Pixels};
use std::io::Cursor;

async fn encoded(image: Image) -> Vec<u8> {
    let mut bytes = Vec::new();
    ImageCodec
        .encode(&image, &mut bytes)
        .await
        .expect("a CPU-resident image encodes");
    bytes
}

async fn decoded(bytes: Vec<u8>) -> Image {
    let byte_len = bytes.len() as u64;
    let value = ImageCodec
        .decode(&mut Cursor::new(bytes), byte_len)
        .await
        .expect("image decodes");
    let image = value
        .as_any()
        .downcast_ref::<Image>()
        .expect("decoded back into a lens Image");
    match &image.pixels {
        Pixels::InterleavedCpu(cpu) => Image::from(cpu.clone()),
        Pixels::PlanarCpu(planar) => Image::from(planar.clone()),
    }
}

/// Every interleaved format comes back with its descriptor and bytes, still
/// interleaved; the header is the layout byte, two format bytes and two
/// little-endian `u64` extents.
#[tokio::test]
async fn every_interleaved_format_round_trips_pixel_exact() {
    for format in ALL_FORMATS {
        let desc = ImageDesc::new(3, 2, format);
        let pixels: Vec<u8> = (0..desc.size_in_bytes())
            .map(|i| (i * 37 % 251) as u8)
            .collect();
        let image = decoded(
            encoded(Image::from(
                CpuImage::new_with_data(desc, pixels.clone()).unwrap(),
            ))
            .await,
        )
        .await;
        let Pixels::InterleavedCpu(cpu) = &image.pixels else {
            panic!("{format} came back planar");
        };
        assert_eq!(cpu.desc(), desc, "{format}");
        assert_eq!(cpu.bytes(), pixels, "{format}");
    }

    let desc = ImageDesc::new(2, 1, ColorFormat::RGB_U8);
    let pixels = vec![10, 20, 30, 40, 50, 60];
    let bytes = encoded(Image::from(
        CpuImage::new_with_data(desc, pixels.clone()).unwrap(),
    ))
    .await;
    assert_eq!(
        &bytes[..HEADER_LEN as usize],
        &[0, 3, 0, 2, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]
    );
    assert_eq!(&bytes[HEADER_LEN as usize..], pixels);
}

/// A planar image keeps its planes: written one after another as
/// little-endian `f32`s and read back planar, so a cached astro output needs
/// no repack either way.
#[tokio::test]
async fn a_planar_image_round_trips_as_planes() {
    let planes = [vec![0.125_f32, 0.5], vec![0.25, 0.625], vec![0.375, 0.75]];
    let planar = LinearImage::from_planar_channels(ImageDimensions::new((2, 1), 3), planes.clone());
    let bytes = encoded(Image::from(planar)).await;
    assert_eq!(&bytes[..3], &[1, 3, 2], "planar, three channels, f32");
    let body: Vec<f32> = bytes[HEADER_LEN as usize..]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    assert_eq!(body, planes.concat(), "plane after plane");

    let image = decoded(bytes).await;
    let Pixels::PlanarCpu(planar) = &image.pixels else {
        panic!("the planes came back interleaved");
    };
    for (channel, plane) in planes.iter().enumerate() {
        assert_eq!(planar.channel(channel).pixels(), plane);
    }

    let gray = LinearImage::from_planar_channels(ImageDimensions::new((2, 1), 1), [vec![0.5, 1.0]]);
    let Pixels::PlanarCpu(gray) = &decoded(encoded(Image::from(gray)).await).await.pixels else {
        panic!("the plane came back interleaved");
    };
    assert_eq!(gray.channel(0).pixels(), &[0.5, 1.0]);
}

/// Each malformed payload is refused by the guard meant for it.
#[tokio::test]
async fn decode_rejects_short_unknown_and_mismatched_payloads() {
    async fn error(bytes: Vec<u8>) -> String {
        let byte_len = bytes.len() as u64;
        ImageCodec
            .decode(&mut Cursor::new(bytes), byte_len)
            .await
            .map(|_| ())
            .expect_err("the payload is refused")
            .to_string()
    }
    let header = |layout: u8, format: [u8; 2], width: u64, height: u64| {
        let mut header = vec![layout, format[0], format[1]];
        header.extend(width.to_le_bytes());
        header.extend(height.to_le_bytes());
        header
    };

    assert_eq!(
        error(vec![0; HEADER_LEN as usize - 1]).await,
        "image cache payload is only 18 bytes"
    );
    for format in [[0, 0], [2, 0], [3, 3]] {
        assert_eq!(
            error(header(0, format, 1, 1)).await,
            "image cache payload names an unknown color format",
            "{format:?}"
        );
    }
    let mut short = header(0, [3, 0], 2, 1);
    short.extend([10, 20, 30, 40, 50]);
    assert_eq!(
        error(short).await,
        "image cache payload has length 24, expected 25"
    );
    assert_eq!(
        error(header(0, [3, 0], u64::MAX, u64::MAX)).await,
        "image cache dimensions overflow memory"
    );
    let mut unknown_layout = header(2, [1, 0], 1, 1);
    unknown_layout.push(0);
    assert_eq!(
        error(unknown_layout).await,
        "image cache payload names layout 2"
    );
    let mut planar_u8 = header(1, [1, 0], 1, 1);
    planar_u8.push(0);
    assert_eq!(
        error(planar_u8).await,
        "planar image cache payload has format L u8"
    );
}

/// The registered type carries this codec.
#[test]
fn register_image_type_wires_the_codec() {
    let mut library = Library::default();
    library.register_type(IMAGE_TYPE_ID, image_type_entry());
    let entry = library.type_entry(IMAGE_TYPE_ID).unwrap();
    assert_eq!(entry.display_name(), "Image");
    assert!(
        format!("{entry:?}").contains("ImageCodec"),
        "the entry holds the codec: {entry:?}"
    );
}
