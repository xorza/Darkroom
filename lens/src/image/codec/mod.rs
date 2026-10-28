//! Streaming disk-cache codec for [`Image`].

use std::borrow::Cow;
use std::io;
use std::sync::Arc;

use async_trait::async_trait;
use common::{Buffer2, DeserializeError, SerdeFormat, USize2};
use imaginarium::{
    ChannelCount, ChannelLayout, ColorFormat, DescError, InterleavedDesc, PlanarImage, SampleType,
};
use lumos::{FlatGain, FrameDescError, FrameGain, ImageMetadata, PixelFlags, frame_desc};
use scenarium::{CodecError, CustomValue, CustomValueCodec, TypeEntry};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use crate::image;
use crate::image::{Image, Pixels};

/// The version of the layout `encode` writes: a cache of another version is rebuilt rather than
/// misread.
const VERSION: u32 = 6;
/// Channel count and sample type, then the width, the height and the record's length as
/// little-endian `u64`s.
const HEADER_LEN: u64 = 2 + 8 + 8 + 8;
/// How many bytes of a plane cross between `f32`s and the stream at once.
const PLANE_CHUNK: usize = 16 * 1024;

#[derive(Debug)]
struct ImageCodec;

/// What an image carries beside its samples, and the order the samples follow in, as bitcode.
#[derive(Debug, Serialize, Deserialize)]
struct ImageRecord<'a> {
    order: SampleOrder,
    metadata: Cow<'a, ImageMetadata>,
    flat_gain: Option<Cow<'a, FlatGain>>,
    /// Whether the flag plane follows the samples.
    flagged: bool,
}

/// How the samples follow the record.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum SampleOrder {
    /// One plane after another, as little-endian `f32`s.
    Planar,
    /// Interleaved, in the bytes the image holds.
    Interleaved,
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
    #[error("image byte count does not fit in the cache format")]
    ByteCountOverflow,
    #[error("image cache payload length overflow")]
    LenOverflow,
    #[error("image cache payload has length {byte_len}, expected {expected}")]
    LenMismatch { byte_len: u64, expected: u64 },
    #[error("invalid cached image: {0}")]
    InvalidDesc(#[from] DescError),
    #[error("invalid cached planes: {0}")]
    InvalidPlanes(#[from] FrameDescError),
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

    /// The header, the record, the samples in the order the record names, then the flag plane.
    async fn encode(
        &self,
        value: &dyn CustomValue,
        writer: &mut (dyn AsyncWrite + Unpin + Send),
    ) -> Result<(), CodecError> {
        let image = value
            .as_any()
            .downcast_ref::<Image>()
            .expect("ImageCodec is only registered for the Image type");
        let record = ImageRecord {
            order: match &image.pixels {
                Pixels::Planar(_) => SampleOrder::Planar,
                Pixels::Interleaved(_) => SampleOrder::Interleaved,
            },
            metadata: Cow::Borrowed(&image.metadata),
            flat_gain: image.flat_gain.as_ref().map(|gain| Cow::Borrowed(&**gain)),
            flagged: image.flags.is_some(),
        };
        let record = SerdeFormat::Bitcode.serialize(&record)?;
        writer
            .write_all(&header(image.desc(), record.len()))
            .await?;
        writer.write_all(&record).await?;
        match &image.pixels {
            Pixels::Interleaved(samples) => writer.write_all(samples.bytes()).await?,
            Pixels::Planar(planes) => {
                let mut chunk = vec![0_u8; PLANE_CHUNK];
                for plane in planes.planes() {
                    write_plane(writer, plane.pixels(), &mut chunk).await?;
                }
            }
        }
        if let Some(flags) = &image.flags {
            writer.write_all(flags.bytes()).await?;
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

        let desc = InterleavedDesc::checked(USize2::new(width, height), color_format)
            .map_err(DecodeError::from)?;
        let pixel_count = desc.size().pixel_count();
        let pixel_len = desc.byte_len();
        let flag_len = if record.flagged { pixel_count } else { 0 };
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
        // The record alone refuses a gain of other pixels, before the samples are read.
        let flat_gain = match record.flat_gain {
            Some(gain) => {
                let gain = gain.into_owned();
                let channels = image::plane_layout(desc.color_format()).count();
                if !gain.covers(desc.size(), channels) {
                    return Err(DecodeError::GainMismatch.into());
                }
                Some(FrameGain::Own(Box::new(gain)))
            }
            None => None,
        };
        let pixels = match record.order {
            SampleOrder::Interleaved => {
                let mut bytes = vec![0; pixel_len];
                reader.read_exact(&mut bytes).await?;
                Pixels::Interleaved(
                    imaginarium::InterleavedImage::new_with_data(desc, bytes)
                        .expect("the payload's length is the descriptor's"),
                )
            }
            SampleOrder::Planar => Pixels::Planar(read_planes(reader, desc).await?),
        };
        let flags = if record.flagged {
            let mut bytes = vec![0; pixel_count];
            reader.read_exact(&mut bytes).await?;
            let flags = PixelFlags::from_bytes(Buffer2::new(desc.size(), bytes))
                .ok_or(DecodeError::EmptyFlags)?;
            Some(flags)
        } else {
            None
        };
        Ok(Arc::new(Image {
            pixels,
            metadata: record.metadata.into_owned(),
            flags,
            flat_gain,
        }))
    }
}

/// The planes of an image of `desc`, read from `reader`.
async fn read_planes(
    reader: &mut (dyn AsyncRead + Unpin + Send),
    desc: InterleavedDesc,
) -> Result<PlanarImage, CodecError> {
    let layout = match desc.color_format() {
        ColorFormat::MONO_F32 => ChannelLayout::Mono,
        ColorFormat::RGB_F32 => ChannelLayout::Rgb,
        color_format => return Err(DecodeError::PlanarFormat(color_format).into()),
    };
    let planar_desc = frame_desc::checked(desc.size(), layout).map_err(DecodeError::from)?;
    let mut chunk = vec![0_u8; PLANE_CHUNK];
    let mut planes = Vec::with_capacity(layout.count());
    for _ in 0..layout.count() {
        planes.push(read_plane(reader, planar_desc.pixel_count(), &mut chunk).await?);
    }
    Ok(PlanarImage::from_planar_channels(planar_desc, planes))
}

/// The header for an image of `desc`, whose record is `record_len` bytes.
fn header(desc: InterleavedDesc, record_len: usize) -> [u8; HEADER_LEN as usize] {
    let mut header = [0; HEADER_LEN as usize];
    header[0..2].copy_from_slice(&format_bytes(desc.color_format()));
    header[2..10].copy_from_slice(&extent(desc.size().width).to_le_bytes());
    header[10..18].copy_from_slice(&extent(desc.size().height).to_le_bytes());
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
        1 => ChannelCount::Mono,
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
