//! Streaming disk-cache codec for [`Image`].

use std::borrow::Cow;
use std::io;
use std::sync::Arc;

use async_trait::async_trait;
use common::{Buffer2, DeserializeError, SerdeFormat};
use imaginarium::{ChannelCount, ColorFormat, ImageDesc, SampleType};
use lumos::{
    ChannelLayout, DimensionsError, FlatGain, FrameGain, ImageDimensions, ImageMetadata,
    LinearImage, PixelFlags,
};
use scenarium::{CodecError, CustomValue, CustomValueCodec, TypeEntry};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use crate::image::{Image, Pixels};

/// The version of the layout `encode` writes: a cache of another version is rebuilt rather than
/// misread.
const VERSION: u32 = 5;
/// Channel count and sample type, then the width, the height and the record's length as
/// little-endian `u64`s.
const HEADER_LEN: u64 = 2 + 8 + 8 + 8;
/// How many bytes of a plane cross between `f32`s and the stream at once.
const PLANE_CHUNK: usize = 16 * 1024;

#[derive(Debug)]
struct ImageCodec;

/// What an image carries beside its samples, in the layout that holds them, as bitcode.
#[derive(Debug, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "one per encode or decode, beside the pixels; boxing the gain would buy an allocation"
)]
enum ImageRecord<'a> {
    /// The samples follow interleaved.
    Interleaved { metadata: Cow<'a, ImageMetadata> },
    /// The planes follow one after another as little-endian `f32`s, then the flag plane when
    /// `flagged`.
    Planar {
        metadata: Cow<'a, ImageMetadata>,
        flat_gain: Option<Cow<'a, FlatGain>>,
        flagged: bool,
    },
}

/// Why a cached image payload does not read back.
#[derive(Debug, thiserror::Error)]
enum DecodeError {
    #[error("image cache payload is only {byte_len} bytes")]
    Short { byte_len: u64 },
    #[error("image cache payload names an unknown color format")]
    UnknownFormat,
    #[error("image cache width does not fit in memory")]
    WidthOverflow,
    #[error("image cache height does not fit in memory")]
    HeightOverflow,
    #[error("image cache record length does not fit in memory")]
    RecordLenOverflow,
    #[error("image cache dimensions overflow memory")]
    DimensionsOverflow,
    #[error("image byte count does not fit in the cache format")]
    ByteCountOverflow,
    #[error("image cache payload length overflow")]
    LenOverflow,
    #[error("image cache payload has length {byte_len}, expected {expected}")]
    LenMismatch { byte_len: u64, expected: u64 },
    #[error("invalid cached image: {0}")]
    InvalidImage(#[from] imaginarium::Error),
    #[error("invalid cached planes: {0}")]
    InvalidPlanes(#[from] DimensionsError),
    #[error("image cache record does not read: {0}")]
    Record(#[from] DeserializeError),
    #[error("image cache payload is flagged, and its flag plane holds no flag")]
    EmptyFlags,
    #[error("image cache flat gain covers another image")]
    GainMismatch,
    #[error("planar image cache payload has format {0}")]
    PlanarFormat(ColorFormat),
}

#[async_trait]
impl CustomValueCodec for ImageCodec {
    fn version(&self) -> u32 {
        VERSION
    }

    /// The header, the record, then the samples in the layout the record names.
    async fn encode(
        &self,
        value: &dyn CustomValue,
        writer: &mut (dyn AsyncWrite + Unpin + Send),
    ) -> Result<(), CodecError> {
        let image = value
            .as_any()
            .downcast_ref::<Image>()
            .expect("ImageCodec is only registered for the Image type");
        let record = match &image.pixels {
            Pixels::InterleavedCpu { metadata, .. } => ImageRecord::Interleaved {
                metadata: Cow::Borrowed(metadata),
            },
            Pixels::PlanarCpu(planar) => ImageRecord::Planar {
                metadata: Cow::Borrowed(&planar.metadata),
                flat_gain: planar.flat_gain().map(|gain| Cow::Borrowed(&**gain)),
                flagged: planar.flags().is_some(),
            },
        };
        let record = SerdeFormat::Bitcode.serialize(&record)?;
        writer
            .write_all(&header(image.desc(), record.len()))
            .await?;
        writer.write_all(&record).await?;
        match &image.pixels {
            Pixels::InterleavedCpu { image, .. } => writer.write_all(image.bytes()).await?,
            Pixels::PlanarCpu(planar) => {
                let mut chunk = vec![0_u8; PLANE_CHUNK];
                for channel in 0..planar.channels() {
                    write_plane(writer, planar.channel(channel).pixels(), &mut chunk).await?;
                }
                if let Some(flags) = planar.flags() {
                    writer.write_all(flags.bytes()).await?;
                }
            }
        }
        Ok(())
    }

    async fn decode(
        &self,
        reader: &mut (dyn AsyncRead + Unpin + Send),
        byte_len: u64,
    ) -> Result<Arc<dyn CustomValue>, CodecError> {
        if byte_len < HEADER_LEN {
            return Err(DecodeError::Short { byte_len }.into());
        }
        let mut header = [0; HEADER_LEN as usize];
        reader.read_exact(&mut header).await?;
        let color_format =
            format_from_bytes([header[0], header[1]]).ok_or(DecodeError::UnknownFormat)?;
        let width = usize::try_from(u64::from_le_bytes(header[2..10].try_into().unwrap()))
            .map_err(|_| DecodeError::WidthOverflow)?;
        let height = usize::try_from(u64::from_le_bytes(header[10..18].try_into().unwrap()))
            .map_err(|_| DecodeError::HeightOverflow)?;
        let record_len = u64::from_le_bytes(header[18..26].try_into().unwrap());
        if record_len > byte_len - HEADER_LEN {
            return Err(DecodeError::Short { byte_len }.into());
        }
        let mut record =
            vec![0; usize::try_from(record_len).map_err(|_| DecodeError::RecordLenOverflow)?];
        reader.read_exact(&mut record).await?;
        let record: ImageRecord<'static> = SerdeFormat::Bitcode
            .deserialize(&record)
            .map_err(DecodeError::from)?;

        let pixel_count = width
            .checked_mul(height)
            .ok_or(DecodeError::DimensionsOverflow)?;
        let pixel_len = pixel_count
            .checked_mul(color_format.byte_count())
            .ok_or(DecodeError::DimensionsOverflow)?;
        let flag_len = match record {
            ImageRecord::Planar { flagged: true, .. } => pixel_count,
            ImageRecord::Interleaved { .. } | ImageRecord::Planar { .. } => 0,
        };
        let expected_len = u64::try_from(pixel_len)
            .ok()
            .zip(u64::try_from(flag_len).ok())
            .ok_or(DecodeError::ByteCountOverflow)
            .and_then(|(pixels, flags)| {
                (HEADER_LEN + record_len)
                    .checked_add(pixels)
                    .and_then(|len| len.checked_add(flags))
                    .ok_or(DecodeError::LenOverflow)
            })?;
        if byte_len != expected_len {
            return Err(DecodeError::LenMismatch {
                byte_len,
                expected: expected_len,
            }
            .into());
        }
        let image = match record {
            ImageRecord::Interleaved { metadata } => {
                let mut bytes = vec![0; pixel_len];
                reader.read_exact(&mut bytes).await?;
                let desc = ImageDesc::new(width, height, color_format);
                Image {
                    pixels: Pixels::InterleavedCpu {
                        image: imaginarium::Image::new_with_data(desc, bytes)
                            .map_err(DecodeError::from)?,
                        metadata: metadata.into_owned(),
                    },
                }
            }
            ImageRecord::Planar {
                metadata,
                flat_gain,
                flagged,
            } => {
                let mut planar = read_planar(reader, color_format, width, height, flagged).await?;
                if let Some(gain) = flat_gain {
                    let gain = gain.into_owned();
                    if !gain.covers(planar.dimensions().size(), planar.channels()) {
                        return Err(DecodeError::GainMismatch.into());
                    }
                    planar.set_flat_gain(Some(FrameGain::Own(Box::new(gain))));
                }
                planar.metadata = metadata.into_owned();
                Image::from(planar)
            }
        };
        Ok(Arc::new(image))
    }
}

/// The planes of a `width × height` image of `color_format`, and its flag plane when `flagged`,
/// read from `reader`.
async fn read_planar(
    reader: &mut (dyn AsyncRead + Unpin + Send),
    color_format: ColorFormat,
    width: usize,
    height: usize,
    flagged: bool,
) -> Result<LinearImage, CodecError> {
    let layout = match color_format {
        ColorFormat::L_F32 => ChannelLayout::Mono,
        ColorFormat::RGB_F32 => ChannelLayout::Rgb,
        _ => return Err(DecodeError::PlanarFormat(color_format).into()),
    };
    let dimensions =
        ImageDimensions::checked((width, height).into(), layout).map_err(DecodeError::from)?;
    let mut chunk = vec![0_u8; PLANE_CHUNK];
    let mut planes = Vec::with_capacity(layout.count());
    for _ in 0..layout.count() {
        planes.push(read_plane(reader, dimensions.pixel_count(), &mut chunk).await?);
    }
    let mut planar = LinearImage::from_planar_channels(dimensions, planes);
    if flagged {
        let mut bytes = vec![0; dimensions.pixel_count()];
        reader.read_exact(&mut bytes).await?;
        let flags = PixelFlags::from_bytes(Buffer2::new(width, height, bytes))
            .ok_or(DecodeError::EmptyFlags)?;
        planar.set_flags(Some(flags));
    }
    Ok(planar)
}

/// The header for an image of `desc`, whose record is `record_len` bytes.
fn header(desc: ImageDesc, record_len: usize) -> [u8; HEADER_LEN as usize] {
    let mut header = [0; HEADER_LEN as usize];
    header[0..2].copy_from_slice(&format_bytes(desc.color_format));
    header[2..10].copy_from_slice(&extent(desc.width).to_le_bytes());
    header[10..18].copy_from_slice(&extent(desc.height).to_le_bytes());
    header[18..26].copy_from_slice(&extent(record_len).to_le_bytes());
    header
}

fn extent(len: usize) -> u64 {
    u64::try_from(len).expect("an extent fits u64")
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
