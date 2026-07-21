use imaginarium::{ColorFormat, Image as CpuImage, ImageDesc, ProcessingContext};
use scenarium::{ContextManager, CustomValueCodec, Library};

use crate::image::codec::{
    ImageCodec, TRAILER_LEN, VERSION, decode_image, encode_image, image_type_entry,
};
use crate::image::context::{VISION_CTX_TYPE, VisionCtx};
use crate::image::{IMAGE_TYPE_ID, Image};

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
async fn cpu_image_round_trips_pixel_exact() {
    let sample = sample();
    let value = Image::from(CpuImage::new_with_data(sample.desc, sample.pixels.clone()).unwrap());
    let blob = ImageCodec
        .encode(&value, &mut cpu_context())
        .await
        .expect("a CPU-resident image encodes");
    assert_eq!(blob.len(), sample.pixels.len() + TRAILER_LEN);

    let decoded = ImageCodec.decode(blob).expect("decodes");
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

#[test]
fn decode_rejects_short_and_future_blobs() {
    assert!(decode_image(vec![0u8; TRAILER_LEN - 1]).is_err());

    let sample = sample();
    let cpu = CpuImage::new_with_data(sample.desc, sample.pixels).unwrap();
    let mut blob = encode_image(&cpu);
    let version_pos = blob.len() - TRAILER_LEN;
    blob[version_pos] = VERSION + 1;
    assert!(decode_image(blob).is_err());
}

#[test]
fn register_image_type_wires_the_codec() {
    let id = *IMAGE_TYPE_ID;
    let mut library = Library::default();
    library.register_type(id, image_type_entry());
    assert!(library.type_decl(&id).is_some());
}
