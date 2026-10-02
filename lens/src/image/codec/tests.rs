use imaginarium::{ALL_FORMATS, ColorFormat, Image as CpuImage, ImageDesc};
use scenarium::{ContextStore, CustomValueCodec, Library};

use crate::image::codec::{HEADER_LEN, ImageCodec, image_type_entry};
use crate::image::{IMAGE_TYPE_ID, Image};
use std::io::Cursor;

#[derive(Debug)]
struct Sample {
    desc: ImageDesc,
    pixels: Vec<u8>,
}

fn sample() -> Sample {
    Sample {
        desc: ImageDesc::new(2, 1, ColorFormat::RGB_U8),
        pixels: vec![10, 20, 30, 40, 50, 60],
    }
}

/// The codec no longer reads anything out of the store — image values are CPU-resident by
/// construction — but the trait still hands one in, so tests need something to pass.
fn cpu_context() -> ContextStore {
    ContextStore::default()
}

async fn round_trip(image: CpuImage) -> CpuImage {
    let value = Image::from(image);
    let mut bytes = Vec::new();
    ImageCodec
        .encode(&value, &mut bytes, &mut cpu_context())
        .await
        .expect("a CPU-resident image encodes");
    let byte_len = bytes.len() as u64;
    let decoded = ImageCodec
        .decode(&mut Cursor::new(bytes), byte_len, &mut cpu_context())
        .await
        .expect("image decodes");
    decoded
        .as_any()
        .downcast_ref::<Image>()
        .expect("decoded back into a lens Image")
        .interleaved()
        .into_owned()
}

/// Every format comes back with its descriptor and bytes; the header is two format bytes and
/// two little-endian `u64` extents.
#[tokio::test]
async fn every_format_round_trips_pixel_exact() {
    for format in ALL_FORMATS {
        let desc = ImageDesc::new(3, 2, format);
        let pixels: Vec<u8> = (0..desc.size_in_bytes())
            .map(|i| (i * 37 % 251) as u8)
            .collect();
        let decoded = round_trip(CpuImage::new_with_data(desc, pixels.clone()).unwrap()).await;
        assert_eq!(decoded.desc(), desc, "{format}");
        assert_eq!(decoded.bytes(), pixels, "{format}");
    }

    let sample = sample();
    let value = Image::from(CpuImage::new_with_data(sample.desc, sample.pixels.clone()).unwrap());
    let mut bytes = Vec::new();
    ImageCodec
        .encode(&value, &mut bytes, &mut cpu_context())
        .await
        .unwrap();
    assert_eq!(
        &bytes[..HEADER_LEN as usize],
        &[3, 0, 2, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]
    );
    assert_eq!(&bytes[HEADER_LEN as usize..], sample.pixels);
}

/// Each malformed payload is refused by the guard meant for it.
#[tokio::test]
async fn decode_rejects_short_unknown_and_mismatched_payloads() {
    async fn error(bytes: Vec<u8>) -> String {
        let byte_len = bytes.len() as u64;
        ImageCodec
            .decode(&mut Cursor::new(bytes), byte_len, &mut cpu_context())
            .await
            .map(|_| ())
            .expect_err("the payload is refused")
            .to_string()
    }
    let header = |format: [u8; 2], width: u64, height: u64| {
        let mut header = format.to_vec();
        header.extend(width.to_le_bytes());
        header.extend(height.to_le_bytes());
        header
    };

    assert_eq!(
        error(vec![0; HEADER_LEN as usize - 1]).await,
        "image cache payload is only 17 bytes"
    );
    for format in [[0, 0], [2, 0], [3, 3]] {
        assert_eq!(
            error(header(format, 1, 1)).await,
            "image cache payload names an unknown color format",
            "{format:?}"
        );
    }
    let mut short = header([3, 0], 2, 1);
    short.extend([10, 20, 30, 40, 50]);
    assert_eq!(
        error(short).await,
        "image cache payload has length 23, expected 24"
    );
    assert_eq!(
        error(header([3, 0], u64::MAX, u64::MAX)).await,
        "image cache dimensions overflow memory"
    );
}

#[test]
fn register_image_type_wires_the_codec() {
    let id = IMAGE_TYPE_ID;
    let mut library = Library::default();
    library.register_type(id, image_type_entry());
    assert!(library.types.contains_key(&id));
}
