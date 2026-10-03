//! Streaming disk-cache codec for [`Image`].

use std::io;
use std::sync::Arc;

use async_trait::async_trait;
use imaginarium::{ChannelCount, ColorFormat, ImageDesc, SampleType};
use lumos::{ImageDimensions, LinearImage};
use scenarium::CodecError;
use scenarium::CustomValue;
use scenarium::CustomValueCodec;
use scenarium::TypeEntry;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use crate::image::{Image, Pixels};

/// 4: a layout byte leads the header, and a planar image keeps its planes.
const VERSION: u32 = 4;
/// Layout, channel count and sample type, then the width and height as
/// little-endian `u64`s.
const HEADER_LEN: u64 = 3 + 8 + 8;
const INTERLEAVED: u8 = 0;
const PLANAR: u8 = 1;
/// How many bytes of a plane cross between `f32`s and the stream at once.
const PLANE_CHUNK: usize = 16 * 1024;

#[derive(Debug)]
struct ImageCodec;

#[async_trait]
impl CustomValueCodec for ImageCodec {
    fn version(&self) -> u32 {
        VERSION
    }

    /// The image in the layout it holds: interleaved samples as they are,
    /// planes one after another as little-endian `f32`s.
    async fn encode(
        &self,
        value: &dyn CustomValue,
        writer: &mut (dyn AsyncWrite + Unpin + Send),
    ) -> Result<(), CodecError> {
        let image = value
            .as_any()
            .downcast_ref::<Image>()
            .expect("ImageCodec is only registered for the Image type");
        let layout = match &image.pixels {
            Pixels::InterleavedCpu(_) => INTERLEAVED,
            Pixels::PlanarCpu(_) => PLANAR,
        };
        writer.write_all(&header(layout, image.desc())).await?;
        match &image.pixels {
            Pixels::InterleavedCpu(cpu) => writer.write_all(cpu.bytes()).await?,
            Pixels::PlanarCpu(planar) => {
                let mut chunk = vec![0_u8; PLANE_CHUNK];
                for channel in 0..planar.channels() {
                    write_plane(writer, planar.channel(channel).pixels(), &mut chunk).await?;
                }
            }
        }
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
    ) -> Result<Arc<dyn CustomValue>, CodecError> {
        if byte_len < HEADER_LEN {
            return Err(format!("image cache payload is only {byte_len} bytes").into());
        }
        let mut header = [0; HEADER_LEN as usize];
        reader.read_exact(&mut header).await?;
        let layout = header[0];
        let color_format = format_from_bytes([header[1], header[2]])
            .ok_or("image cache payload names an unknown color format")?;
        let width = usize::try_from(u64::from_le_bytes(header[3..11].try_into().unwrap()))
            .map_err(|_| "image cache width does not fit in memory")?;
        let height = usize::try_from(u64::from_le_bytes(header[11..19].try_into().unwrap()))
            .map_err(|_| "image cache height does not fit in memory")?;
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
        let image = match layout {
            INTERLEAVED => {
                let mut bytes = Vec::with_capacity(pixel_len);
                (&mut *reader)
                    .take(expected_len - HEADER_LEN)
                    .read_to_end(&mut bytes)
                    .await?;
                if bytes.len() != pixel_len {
                    return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into());
                }
                let desc = ImageDesc::new(width, height, color_format);
                Image::from(
                    imaginarium::Image::new_with_data(desc, bytes)
                        .map_err(|error| format!("invalid cached image: {error:?}"))?,
                )
            }
            PLANAR => {
                let channels = match color_format {
                    ColorFormat::L_F32 => 1,
                    ColorFormat::RGB_F32 => 3,
                    _ => {
                        return Err(format!(
                            "planar image cache payload has format {color_format}"
                        )
                        .into());
                    }
                };
                let mut chunk = vec![0_u8; PLANE_CHUNK];
                let mut planes = Vec::with_capacity(channels);
                for _ in 0..channels {
                    planes.push(read_plane(reader, width * height, &mut chunk).await?);
                }
                Image::from(LinearImage::from_planar_channels(
                    ImageDimensions::new((width, height), channels),
                    planes,
                ))
            }
            _ => return Err(format!("image cache payload names layout {layout}").into()),
        };
        Ok(Arc::new(image))
    }
}

/// The header for an image of `desc` in `layout`.
fn header(layout: u8, desc: ImageDesc) -> [u8; HEADER_LEN as usize] {
    let mut header = [0; HEADER_LEN as usize];
    header[0] = layout;
    header[1..3].copy_from_slice(&format_bytes(desc.color_format));
    header[3..11].copy_from_slice(&(desc.width as u64).to_le_bytes());
    header[11..19].copy_from_slice(&(desc.height as u64).to_le_bytes());
    header
}

/// One plane as little-endian `f32`s, a `chunk` at a time.
async fn write_plane(
    writer: &mut (dyn AsyncWrite + Unpin + Send),
    plane: &[f32],
    chunk: &mut [u8],
) -> io::Result<()> {
    for samples in plane.chunks(PLANE_CHUNK / size_of::<f32>()) {
        let (words, _) = chunk.as_chunks_mut::<{ size_of::<f32>() }>();
        for (bytes, sample) in words.iter_mut().zip(samples) {
            *bytes = sample.to_le_bytes();
        }
        writer.write_all(&chunk[..size_of_val(samples)]).await?;
    }
    Ok(())
}

/// `count` little-endian `f32`s into a plane allocated once at its size, a
/// `chunk` at a time.
async fn read_plane(
    reader: &mut (dyn AsyncRead + Unpin + Send),
    count: usize,
    chunk: &mut [u8],
) -> io::Result<Vec<f32>> {
    let mut plane = Vec::with_capacity(count);
    let mut remaining = count * size_of::<f32>();
    while remaining > 0 {
        let len = remaining.min(PLANE_CHUNK);
        reader.read_exact(&mut chunk[..len]).await?;
        plane.extend(
            chunk[..len]
                .as_chunks::<{ size_of::<f32>() }>()
                .0
                .iter()
                .map(|bytes| f32::from_le_bytes(*bytes)),
        );
        remaining -= len;
    }
    Ok(plane)
}

/// The two header bytes that name a format: its channel count, and its sample type as 0, 1, 2.
const fn format_bytes(format: ColorFormat) -> [u8; 2] {
    let sample = match format.sample_type {
        SampleType::U8 => 0,
        SampleType::U16 => 1,
        SampleType::F32 => 2,
    };
    [format.channel_count as u8, sample]
}

const fn format_from_bytes(bytes: [u8; 2]) -> Option<ColorFormat> {
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
