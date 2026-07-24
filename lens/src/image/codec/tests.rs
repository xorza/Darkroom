use imaginarium::{ColorFormat, Image as CpuImage, ImageDesc, ProcessingContext};
use scenarium::{ContextManager, CustomValueCodec, Library};

use crate::image::codec::{HEADER_LEN, ImageCodec, image_type_entry};
use crate::image::context::{VISION_CTX_TYPE, VisionCtx};
use crate::image::{IMAGE_TYPE_ID, Image};

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

fn cpu_context() -> ContextManager {
    let mut context = ContextManager::default();
    scenarium::insert_context(
        &mut context,
        &VISION_CTX_TYPE,
        VisionCtx {
            processing_ctx: ProcessingContext::cpu_only(),
        },
    );
    context
}

#[tokio::test]
async fn cpu_image_streams_round_trip_pixel_exact() {
    let sample = sample();
    let value = Image::from(CpuImage::new_with_data(sample.desc, sample.pixels.clone()).unwrap());
    let mut bytes = Vec::new();
    ImageCodec
        .encode(&value, &mut bytes, &mut cpu_context())
        .await
        .expect("a CPU-resident image encodes");
    assert_eq!(bytes.len(), sample.pixels.len() + HEADER_LEN as usize);

    let byte_len = bytes.len() as u64;
    let mut reader = std::io::Cursor::new(bytes);
    let decoded = ImageCodec
        .decode(&mut reader, byte_len)
        .await
        .expect("image decodes");
    let decoded = decoded
        .as_any()
        .downcast_ref::<Image>()
        .expect("decoded back into a lens Image");
    let cpu = decoded
        .buffer
        .make_cpu(&ProcessingContext::cpu_only())
        .expect("rebuilt image is CPU-resident");
    assert_eq!(cpu.desc(), sample.desc);
    assert_eq!(cpu.bytes(), sample.pixels);
}

#[tokio::test]
async fn decode_rejects_short_unknown_and_mismatched_payloads() {
    for bytes in [
        vec![0; HEADER_LEN as usize - 1],
        vec![0; HEADER_LEN as usize],
    ] {
        let byte_len = bytes.len() as u64;
        assert!(
            ImageCodec
                .decode(&mut std::io::Cursor::new(bytes), byte_len)
                .await
                .is_err()
        );
    }

    let sample = sample();
    let value = Image::from(CpuImage::new_with_data(sample.desc, sample.pixels).unwrap());
    let mut bytes = Vec::new();
    ImageCodec
        .encode(&value, &mut bytes, &mut cpu_context())
        .await
        .unwrap();
    bytes.pop();
    let byte_len = bytes.len() as u64;
    assert!(
        ImageCodec
            .decode(&mut std::io::Cursor::new(bytes), byte_len)
            .await
            .is_err()
    );

    let mut overflowing = vec![0; HEADER_LEN as usize];
    overflowing[0] = ColorFormat::RGB_U8.channel_count as u8;
    overflowing[1] = ColorFormat::RGB_U8.channel_size as u8;
    overflowing[2] = ColorFormat::RGB_U8.channel_type as u8;
    overflowing[3..11].copy_from_slice(&u64::MAX.to_le_bytes());
    overflowing[11..19].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(
        ImageCodec
            .decode(
                &mut std::io::Cursor::new(&overflowing),
                overflowing.len() as u64,
            )
            .await
            .is_err()
    );
}

#[test]
fn register_image_type_wires_the_codec() {
    let id = *IMAGE_TYPE_ID;
    let mut library = Library::default();
    library.register_type(id, image_type_entry());
    assert!(library.types.contains_key(&id));
}
