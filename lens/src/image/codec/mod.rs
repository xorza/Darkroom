//! Disk codec for [`Image`] — the [`CustomValueCodec`] that lets the scenarium
//! output cache persist images. The blob is the raw packed pixel bytes followed
//! by a small fixed trailer (format + dimensions); see
//! `scenarium/docs/disk-cache-design.md`.

use std::sync::Arc;

use async_trait::async_trait;
use imaginarium::{ALL_FORMATS, ImageDesc};
use scenarium::ContextManager;
use scenarium::CustomValue;
use scenarium::CustomValueCodec;
use scenarium::TypeEntry;

use crate::image::Image;
use crate::image::context::{VISION_CTX_TYPE, VisionCtx};

/// On-disk layout version for an image blob. Bump on any layout change.
const VERSION: u8 = 1;

/// Fixed trailer: `version(1) + channel_count(1) + channel_size(1) +
/// channel_type(1) + width(8) + height(8)`. It lives at the *end* of the blob so
/// the decoder can drop it with `Vec::truncate` and hand the pixel prefix to
/// `Image::new_with_data` without copying it again.
const TRAILER_LEN: usize = 1 + 3 + 8 + 8;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The [`Image`] codec (a ZST — all state is in the blob).
#[derive(Debug)]
struct ImageCodec;

#[async_trait]
impl CustomValueCodec for ImageCodec {
    async fn encode(
        &self,
        value: &dyn CustomValue,
        ctx: &mut ContextManager,
    ) -> std::result::Result<Vec<u8>, BoxError> {
        let image = value
            .as_any()
            .downcast_ref::<Image>()
            .expect("ImageCodec is only registered for the Image type");
        let vision = ctx.get::<VisionCtx>(&VISION_CTX_TYPE);
        // Instant for a CPU-resident image, a bounded GPU→CPU download otherwise
        // — fine to run inline for a one-shot cache write.
        let cpu = image
            .buffer
            .make_cpu(&vision.processing_ctx)
            .map_err(|e| format!("image gpu readback failed: {e:?}"))?;
        Ok(encode_image(&cpu))
    }

    fn decode(&self, blob: Vec<u8>) -> std::result::Result<Arc<dyn CustomValue>, BoxError> {
        decode_image(blob)
    }
}

/// The [`Image`] type's registry entry (display name + disk codec), for
/// `image_library` to register via `Library::register_type`. Keeps the codec
/// (a private ZST) owned here; the caller supplies only `IMAGE_TYPE_ID`.
pub(crate) fn image_type_entry() -> TypeEntry {
    TypeEntry::custom_with_codec("Image", Arc::new(ImageCodec))
}

fn encode_image(image: &imaginarium::Image) -> Vec<u8> {
    let desc = image.desc();
    let format = desc.color_format;
    let pixels = image.bytes();

    let mut blob = Vec::with_capacity(pixels.len() + TRAILER_LEN);
    blob.extend_from_slice(pixels);
    blob.push(VERSION);
    blob.push(format.channel_count as u8);
    blob.push(format.channel_size as u8);
    blob.push(format.channel_type as u8);
    blob.extend_from_slice(&(desc.width as u64).to_le_bytes());
    blob.extend_from_slice(&(desc.height as u64).to_le_bytes());
    blob
}

fn decode_image(mut blob: Vec<u8>) -> std::result::Result<Arc<dyn CustomValue>, BoxError> {
    if blob.len() < TRAILER_LEN {
        return Err(format!("image cache blob too short: {} bytes", blob.len()).into());
    }
    let pixel_len = blob.len() - TRAILER_LEN;
    let trailer = &blob[pixel_len..];

    if trailer[0] != VERSION {
        return Err(format!("unsupported image cache version {}", trailer[0]).into());
    }
    let (count, size, ty) = (trailer[1], trailer[2], trailer[3]);
    let color_format = ALL_FORMATS
        .iter()
        .copied()
        .find(|f| {
            f.channel_count as u8 == count
                && f.channel_size as u8 == size
                && f.channel_type as u8 == ty
        })
        .ok_or("image cache blob names an unknown color format")?;
    let width = u64::from_le_bytes(trailer[4..12].try_into().unwrap()) as usize;
    let height = u64::from_le_bytes(trailer[12..20].try_into().unwrap()) as usize;
    let desc = ImageDesc::new(width, height, color_format);

    // Drop the trailer in place; the pixel prefix is left untouched so
    // `new_with_data` (which revalidates the length) gets it without a recopy.
    blob.truncate(pixel_len);
    let image = imaginarium::Image::new_with_data(desc, blob)
        .map_err(|e| format!("malformed image cache blob: {e:?}"))?;
    Ok(Arc::new(Image::from(image)))
}

#[cfg(test)]
mod tests;
