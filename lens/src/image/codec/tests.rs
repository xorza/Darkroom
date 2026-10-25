use std::borrow::Cow;
use std::io::Cursor;

use common::{Buffer2, SerdeFormat};
use imaginarium::{ALL_FORMATS, ColorFormat, Image as CpuImage, ImageDesc};
use lumos::{
    ChannelLayout, FlatGain, FrameGain, ImageDimensions, ImageMetadata, LinearImage, PixelFlags,
};
use scenarium::{CustomValue, CustomValueCodec, Library};

use crate::image::codec::{HEADER_LEN, ImageCodec, ImageRecord, image_type_entry};
use crate::image::{IMAGE_TYPE_ID, Image, Pixels};

fn header_len() -> usize {
    usize::try_from(HEADER_LEN).unwrap()
}

async fn encoded(image: Image) -> Vec<u8> {
    let mut bytes = Vec::new();
    ImageCodec
        .encode(&image, &mut bytes)
        .await
        .expect("a CPU-resident image encodes");
    bytes
}

async fn decoded(bytes: Vec<u8>) -> Image {
    let byte_len = u64::try_from(bytes.len()).unwrap();
    let value = ImageCodec
        .decode(&mut Cursor::new(bytes), byte_len)
        .await
        .expect("image decodes");
    let image = value
        .as_any()
        .downcast_ref::<Image>()
        .expect("decoded back into a lens Image");
    let pixels = match &image.pixels {
        Pixels::InterleavedCpu { image, metadata } => Pixels::InterleavedCpu {
            image: image.clone(),
            metadata: metadata.clone(),
        },
        Pixels::PlanarCpu(planar) => Pixels::PlanarCpu(planar.clone()),
    };
    Image { pixels }
}

/// The record's length, from the header's last `u64`.
fn record_len(bytes: &[u8]) -> usize {
    usize::try_from(u64::from_le_bytes(bytes[18..26].try_into().unwrap())).unwrap()
}

/// Metadata a test tells apart from the default: an object name.
fn named(object: &str) -> ImageMetadata {
    let mut metadata = ImageMetadata::default();
    metadata.header.object = Some(object.to_owned());
    metadata
}

/// The 2×1 one-channel gain of nodes 1.5 and 2: a 2-pixel row takes nodes at 0 and 4, a 1-pixel
/// column one node.
fn gain() -> FlatGain {
    SerdeFormat::Ron
        .deserialize(
            b"(width: 2, height: 1, nodes: [[1.5, 2.0]], \
              bins: [(bounds: (1.5, 1.5, 1.5, 2.0, 2.0, 2.0, 2.0))])",
        )
        .unwrap()
}

/// Every interleaved format comes back with its descriptor, bytes and metadata, still
/// interleaved; the header is two format bytes, then the width, the height and the record's length
/// as little-endian `u64`s.
#[tokio::test]
async fn every_interleaved_format_round_trips_pixel_exact() {
    for format in ALL_FORMATS {
        let desc = ImageDesc::new(3, 2, format);
        let pixels: Vec<u8> = (0..desc.size_in_bytes())
            .map(|i| u8::try_from(i * 37 % 251).unwrap())
            .collect();
        let image = Image {
            pixels: Pixels::InterleavedCpu {
                image: CpuImage::new_with_data(desc, pixels.clone()).unwrap(),
                metadata: named("M31"),
            },
        };
        let image = decoded(encoded(image).await).await;
        let Pixels::InterleavedCpu { image, metadata } = &image.pixels else {
            panic!("{format} came back planar");
        };
        assert_eq!(image.desc(), desc, "{format}");
        assert_eq!(image.bytes(), pixels, "{format}");
        assert_eq!(metadata.header.object.as_deref(), Some("M31"), "{format}");
    }

    let desc = ImageDesc::new(2, 1, ColorFormat::RGB_U8);
    let pixels = vec![10, 20, 30, 40, 50, 60];
    let bytes = encoded(Image::from(
        CpuImage::new_with_data(desc, pixels.clone()).unwrap(),
    ))
    .await;
    assert_eq!(
        &bytes[..18],
        &[3, 0, 2, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]
    );
    assert_eq!(
        bytes.len(),
        header_len() + record_len(&bytes) + pixels.len()
    );
    assert_eq!(&bytes[header_len() + record_len(&bytes)..], pixels);
}

/// A planar image keeps its planes, its flags, its flat gain and its metadata: the planes written
/// one after another as little-endian `f32`s and read back planar, so a cached astro output needs
/// no repack either way, then the flag plane.
#[tokio::test]
async fn a_planar_image_round_trips_whole() {
    let planes = [vec![0.125_f32, 0.5], vec![0.25, 0.625], vec![0.375, 0.75]];
    let planar = LinearImage::from_planar_channels(
        ImageDimensions::new((2, 1), ChannelLayout::Rgb),
        planes.clone(),
    );
    let bytes = encoded(Image::from(planar)).await;
    assert_eq!(&bytes[..2], &[3, 2], "three channels, f32");
    let body: Vec<f32> = bytes[header_len() + record_len(&bytes)..]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    assert_eq!(
        body,
        planes.concat(),
        "plane after plane, and no flag plane"
    );

    let image = decoded(bytes).await;
    let Pixels::PlanarCpu(planar) = &image.pixels else {
        panic!("the planes came back interleaved");
    };
    for (channel, plane) in planes.iter().enumerate() {
        assert_eq!(planar.channel(channel).pixels(), plane);
    }
    assert!(planar.flags().is_none() && planar.flat_gain().is_none());

    let mut mono = LinearImage::from_planar_channels(
        ImageDimensions::new((2, 1), ChannelLayout::Mono),
        [vec![0.5, 1.0]],
    );
    mono.metadata = named("NGC 7000");
    mono.set_flags(PixelFlags::from_bytes(Buffer2::new(2, 1, vec![0, 5])));
    mono.set_flat_gain(Some(FrameGain::Own(Box::new(gain()))));
    let mono = Image::from(mono);
    // Two `f32` samples, two flag bytes, and the gain's 2 nodes of one channel, 4 bytes each.
    assert_eq!(mono.ram_bytes().cpu, 8 + 2 + 8);
    let bytes = encoded(mono).await;
    assert_eq!(
        &bytes[bytes.len() - 2..],
        &[0, 5],
        "the flag plane comes last"
    );
    let decoded = decoded(bytes).await;
    assert_eq!(
        decoded.ram_bytes().cpu,
        8 + 2 + 8,
        "the read-back gain is the image's own"
    );
    let Pixels::PlanarCpu(mono) = &decoded.pixels else {
        panic!("the plane came back interleaved");
    };
    assert_eq!(mono.channel(0).pixels(), &[0.5, 1.0]);
    assert_eq!(mono.metadata.header.object.as_deref(), Some("NGC 7000"));
    assert_eq!(mono.flags().map(PixelFlags::bytes), Some(&[0, 5][..]));
    assert_eq!(
        mono.flat_gain()
            .map(|gain| SerdeFormat::Ron.serialize(&**gain).unwrap()),
        Some(SerdeFormat::Ron.serialize(&gain()).unwrap())
    );
}

/// Each malformed payload is refused by the guard meant for it.
#[tokio::test]
async fn decode_rejects_short_unknown_and_mismatched_payloads() {
    async fn error(bytes: Vec<u8>) -> String {
        let byte_len = u64::try_from(bytes.len()).unwrap();
        ImageCodec
            .decode(&mut Cursor::new(bytes), byte_len)
            .await
            .map(|_| ())
            .expect_err("the payload is refused")
            .to_string()
    }
    let raster = SerdeFormat::Bitcode
        .serialize(&ImageRecord::Interleaved {
            metadata: Cow::Owned(ImageMetadata::default()),
        })
        .unwrap();
    let planes = |flat_gain: Option<FlatGain>, flagged: bool| {
        SerdeFormat::Bitcode
            .serialize(&ImageRecord::Planar {
                metadata: Cow::Owned(ImageMetadata::default()),
                flat_gain: flat_gain.map(Cow::Owned),
                flagged,
            })
            .unwrap()
    };
    let payload = |format: [u8; 2], width: u64, height: u64, record: &[u8]| {
        let mut payload = format.to_vec();
        payload.extend(width.to_le_bytes());
        payload.extend(height.to_le_bytes());
        payload.extend(u64::try_from(record.len()).unwrap().to_le_bytes());
        payload.extend(record);
        payload
    };

    assert_eq!(
        error(vec![0; header_len() - 1]).await,
        "image cache payload is only 25 bytes"
    );
    for format in [[0, 0], [2, 0], [3, 3]] {
        assert_eq!(
            error(payload(format, 1, 1, &raster)).await,
            "image cache payload names an unknown color format",
            "{format:?}"
        );
    }
    let mut overlong = payload([3, 0], 1, 1, &raster);
    overlong[18] = u8::MAX;
    assert_eq!(
        error(overlong.clone()).await,
        format!("image cache payload is only {} bytes", overlong.len())
    );
    assert!(
        error(payload([3, 0], 1, 1, &[0xFF]))
            .await
            .starts_with("image cache record does not read")
    );
    let mut short = payload([3, 0], 2, 1, &raster);
    short.extend([10, 20, 30, 40, 50]);
    let expected = header_len() + raster.len() + 6;
    assert_eq!(
        error(short).await,
        format!(
            "image cache payload has length {}, expected {expected}",
            expected - 1
        )
    );
    assert_eq!(
        error(payload([3, 0], u64::MAX, u64::MAX, &raster)).await,
        "image cache dimensions overflow memory"
    );
    let mut planar_u8 = payload([1, 0], 1, 1, &planes(None, false));
    planar_u8.push(0);
    assert_eq!(
        error(planar_u8).await,
        "planar image cache payload has format L u8"
    );
    let mut empty_flags = payload([1, 2], 2, 1, &planes(None, true));
    empty_flags.extend([0; 8]);
    empty_flags.extend([0, 0]);
    assert_eq!(
        error(empty_flags).await,
        "image cache payload is flagged, and its flag plane holds no flag"
    );
    let mut other_gain = payload([1, 2], 3, 1, &planes(Some(gain()), false));
    other_gain.extend([0; 12]);
    assert_eq!(
        error(other_gain).await,
        "image cache flat gain covers another image"
    );
    let zero_side = payload([1, 2], 0, 1, &planes(None, false));
    assert!(error(zero_side).await.starts_with("invalid cached planes"));
}

/// Bitcode is not self-describing, so a record from another layout would decode into plausible
/// nonsense. A change of the layout fails this until the pin moves with a bump of `VERSION`.
#[test]
fn record_layout_is_pinned() {
    let bytes = SerdeFormat::Bitcode
        .serialize(&ImageRecord::Planar {
            metadata: Cow::Owned(named("M31")),
            flat_gain: Some(Cow::Owned(gain())),
            flagged: true,
        })
        .unwrap();
    assert_eq!(
        &blake3::hash(&bytes).to_hex()[..16],
        "53694879ac395f3a",
        "the record layout moved: bump VERSION and set this pin to the digest"
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
