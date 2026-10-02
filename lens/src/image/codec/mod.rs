//! Streaming disk-cache codec for [`Image`].

use std::error;
use std::sync::Arc;

use async_trait::async_trait;
use imaginarium::{ChannelCount, ColorFormat, ImageDesc, SampleType};
use scenarium::ContextStore;
use scenarium::CustomValue;
use scenarium::CustomValueCodec;
use scenarium::TypeEntry;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use crate::image::Image;

/// 3: the format is two bytes, channel count and sample type.
const VERSION: u32 = 3;
const HEADER_LEN: u64 = 2 + 8 + 8;

type BoxError = Box<dyn error::Error + Send + Sync>;

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
        _ctx: &mut ContextStore,
    ) -> Result<(), BoxError> {
        let image = value
            .as_any()
            .downcast_ref::<Image>()
            .expect("ImageCodec is only registered for the Image type");
        let cpu = image.interleaved();
        let desc = cpu.desc();
        let format = desc.color_format;
        let mut header = [0; HEADER_LEN as usize];
        header[..2].copy_from_slice(&format_bytes(format));
        header[2..10].copy_from_slice(&(desc.width as u64).to_le_bytes());
        header[10..18].copy_from_slice(&(desc.height as u64).to_le_bytes());
        writer.write_all(&header).await?;
        writer.write_all(cpu.bytes()).await?;
        Ok(())
    }

    #[expect(
        clippy::map_err_ignore,
        reason = "a `TryFromIntError` says only that the value does not fit, which the error it becomes states"
    )]
    async fn decode(
        &self,
        reader: &mut (dyn AsyncRead + Unpin + Send),
        byte_len: u64,
        _ctx: &mut ContextStore,
    ) -> Result<Arc<dyn CustomValue>, BoxError> {
        if byte_len < HEADER_LEN {
            return Err(format!("image cache payload is only {byte_len} bytes").into());
        }
        let mut header = [0; HEADER_LEN as usize];
        reader.read_exact(&mut header).await?;
        let color_format = format_from_bytes([header[0], header[1]])
            .ok_or("image cache payload names an unknown color format")?;
        let width = usize::try_from(u64::from_le_bytes(header[2..10].try_into().unwrap()))
            .map_err(|_| "image cache width does not fit in memory")?;
        let height = usize::try_from(u64::from_le_bytes(header[10..18].try_into().unwrap()))
            .map_err(|_| "image cache height does not fit in memory")?;
        let desc = ImageDesc::new(width, height, color_format);
        let pixel_len = width
            .checked_mul(height)
            .and_then(|pixel_count| pixel_count.checked_mul(color_format.byte_count()))
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

/// The two header bytes that name a format: its channel count, and its sample type as 0, 1, 2.
fn format_bytes(format: ColorFormat) -> [u8; 2] {
    let sample = match format.sample_type {
        SampleType::U8 => 0,
        SampleType::U16 => 1,
        SampleType::F32 => 2,
    };
    [format.channel_count as u8, sample]
}

fn format_from_bytes(bytes: [u8; 2]) -> Option<ColorFormat> {
    let channel_count = match bytes[0] {
        1 => ChannelCount::L,
        3 => ChannelCount::Rgb,
        4 => ChannelCount::Rgba,
        _ => return None,
    };
    let sample_type = match bytes[1] {
        0 => SampleType::U8,
        1 => SampleType::U16,
        2 => SampleType::F32,
        _ => return None,
    };
    Some(ColorFormat::new(channel_count, sample_type))
}

pub(super) fn image_type_entry() -> TypeEntry {
    TypeEntry::custom_with_codec("Image", Arc::new(ImageCodec))
}

#[cfg(test)]
mod tests;
