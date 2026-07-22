//! Streaming disk-cache codec for [`Image`].

use std::sync::Arc;

use async_trait::async_trait;
use imaginarium::{ALL_FORMATS, ImageDesc};
use scenarium::ContextManager;
use scenarium::CustomValue;
use scenarium::CustomValueCodec;
use scenarium::TypeEntry;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use crate::image::Image;
use crate::image::context::{VISION_CTX_TYPE, VisionCtx};

const VERSION: u32 = 2;
const HEADER_LEN: u64 = 3 + 8 + 8;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug)]
struct ImageCodec;

#[async_trait]
impl CustomValueCodec for ImageCodec {
    fn version(&self) -> u32 {
        VERSION
    }

    async fn encode(
        &self,
        value: &dyn CustomValue,
        writer: &mut (dyn AsyncWrite + Unpin + Send),
        ctx: &mut ContextManager,
    ) -> std::result::Result<(), BoxError> {
        let image = value
            .as_any()
            .downcast_ref::<Image>()
            .expect("ImageCodec is only registered for the Image type");
        let vision = ctx.get::<VisionCtx>(&VISION_CTX_TYPE);
        let cpu = image
            .buffer
            .make_cpu(&vision.processing_ctx)
            .map_err(|error| format!("image GPU readback failed: {error:?}"))?;
        let desc = cpu.desc();
        let format = desc.color_format;
        let mut header = [0; HEADER_LEN as usize];
        header[0] = format.channel_count as u8;
        header[1] = format.channel_size as u8;
        header[2] = format.channel_type as u8;
        header[3..11].copy_from_slice(&(desc.width as u64).to_le_bytes());
        header[11..19].copy_from_slice(&(desc.height as u64).to_le_bytes());
        writer.write_all(&header).await?;
        writer.write_all(cpu.bytes()).await?;
        Ok(())
    }

    async fn decode(
        &self,
        reader: &mut (dyn AsyncRead + Unpin + Send),
        byte_len: u64,
    ) -> std::result::Result<Arc<dyn CustomValue>, BoxError> {
        if byte_len < HEADER_LEN {
            return Err(format!("image cache payload is only {byte_len} bytes").into());
        }
        let mut header = [0; HEADER_LEN as usize];
        reader.read_exact(&mut header).await?;
        let color_format = ALL_FORMATS
            .iter()
            .copied()
            .find(|format| {
                format.channel_count as u8 == header[0]
                    && format.channel_size as u8 == header[1]
                    && format.channel_type as u8 == header[2]
            })
            .ok_or("image cache payload names an unknown color format")?;
        let width = usize::try_from(u64::from_le_bytes(header[3..11].try_into().unwrap()))
            .map_err(|_| "image cache width does not fit in memory")?;
        let height = usize::try_from(u64::from_le_bytes(header[11..19].try_into().unwrap()))
            .map_err(|_| "image cache height does not fit in memory")?;
        let desc = ImageDesc::new(width, height, color_format);
        let pixel_len = width
            .checked_mul(height)
            .and_then(|pixel_count| pixel_count.checked_mul(color_format.byte_count() as usize))
            .ok_or("image cache dimensions overflow memory")?;
        let expected_len = HEADER_LEN
            .checked_add(
                u64::try_from(pixel_len)
                    .map_err(|_| "image byte count does not fit in the cache format")?,
            )
            .ok_or("image cache payload length overflow")?;
        if byte_len != expected_len {
            return Err(format!(
                "image cache payload has length {byte_len}, expected {expected_len}"
            )
            .into());
        }
        let mut image = imaginarium::Image::new_black(desc)
            .map_err(|error| format!("invalid cached image descriptor: {error:?}"))?;
        reader.read_exact(image.bytes_mut()).await?;
        Ok(Arc::new(Image::from(image)))
    }
}

pub(crate) fn image_type_entry() -> TypeEntry {
    TypeEntry::custom_with_codec("Image", Arc::new(ImageCodec))
}

#[cfg(test)]
mod tests;
