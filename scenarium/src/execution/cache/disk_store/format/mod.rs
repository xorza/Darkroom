//! Indexed binary format for streamed node-output cache blobs.

use std::io::{self, SeekFrom};

use tokio::io::Take;
use tokio::io::{
    AsyncRead, AsyncReadExt as _, AsyncSeek, AsyncSeekExt as _, AsyncWrite, AsyncWriteExt as _,
};

use crate::data::codec::Codecs;
use crate::data::const_value::ConstValue;
use crate::data::data_type::TypeId;
use crate::data::dynamic_value::DynamicValue;
use crate::execution::cache::digest::Digest;
use crate::execution::cache::disk_store::error::{CodecFormatError, FormatResult, FrameError};
use crate::graph::func::lambda::OutputDemand;

const MAGIC: &[u8; 8] = b"SCENBLOB";
const FORMAT_VERSION: u32 = 10;
const FIXED_LEN: usize = 8 + 4 + 32 + 4 + 8;
const DESCRIPTOR_LEN: usize = 1 + 3 + 16 + 4 + 8;
const BODY_LEN_OFFSET: u64 = (8 + 4 + 32 + 4) as u64;
const PAYLOAD_LEN_OFFSET: u64 = (1 + 3 + 16 + 4) as u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputKind {
    Unbound,
    Static,
    Custom { type_id: TypeId, version: u32 },
}

impl OutputKind {
    const fn tag(self) -> u8 {
        match self {
            Self::Unbound => 0,
            Self::Static => 1,
            Self::Custom { .. } => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OutputDescriptor {
    kind: OutputKind,
    payload_len: u64,
}

#[derive(Clone, Copy, Debug)]
struct HeaderPrefix {
    output_count: usize,
    body_len: u64,
}

#[derive(Debug)]
struct BlobHeader {
    descriptors: Vec<OutputDescriptor>,
}

/// A blob being written: the stream, the digest it is stored under, and the
/// codecs its custom values encode with.
#[derive(Debug)]
pub(super) struct BlobWriter<'a, W> {
    pub(super) writer: &'a mut W,
    pub(super) digest: Digest,
    pub(super) codecs: &'a Codecs,
}

impl<W> BlobWriter<'_, W>
where
    W: AsyncWrite + AsyncSeek + Unpin + Send,
{
    pub(super) async fn write(self, outputs: &[DynamicValue]) -> FormatResult<()> {
        let Self {
            writer,
            digest,
            codecs,
        } = self;
        let output_count =
            u32::try_from(outputs.len()).expect("a node output count must fit in the cache format");
        let mut fixed = [0; FIXED_LEN];
        fixed[..8].copy_from_slice(MAGIC);
        fixed[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        fixed[12..44].copy_from_slice(&digest.0);
        fixed[44..48].copy_from_slice(&output_count.to_le_bytes());
        writer.write_all(&fixed).await?;

        for value in outputs {
            write_descriptor(writer, descriptor_for(value, codecs)?).await?;
        }

        let body_start = u64::try_from(header_len(outputs.len()))
            .expect("a cache header length must fit in the file format");
        debug_assert_eq!(writer.stream_position().await?, body_start);

        for (idx, value) in outputs.iter().enumerate() {
            if matches!(value, DynamicValue::Unbound) {
                continue;
            }
            let payload_start = writer.stream_position().await?;
            match value {
                DynamicValue::Unbound => unreachable!(),
                DynamicValue::Static(value) => write_static(writer, value).await?,
                DynamicValue::Custom(value) => {
                    let type_id = value.type_id();
                    let codec = codecs
                        .get(type_id)
                        .expect("custom output codec was checked while writing descriptors");
                    codec
                        .encode(value.as_ref(), writer)
                        .await
                        .map_err(|source| CodecFormatError::Encode { type_id, source })?;
                }
            }
            let payload_end = writer.stream_position().await?;
            let payload_len = payload_end
                .checked_sub(payload_start)
                .expect("a streaming cache writer cannot move before its payload start");
            writer
                .seek(SeekFrom::Start(descriptor_payload_len_offset(idx)))
                .await?;
            writer.write_all(&payload_len.to_le_bytes()).await?;
            writer.seek(SeekFrom::Start(payload_end)).await?;
        }

        let body_end = writer.stream_position().await?;
        let body_len = body_end
            .checked_sub(body_start)
            .expect("a streaming cache writer cannot move before its body start");
        writer.seek(SeekFrom::Start(BODY_LEN_OFFSET)).await?;
        writer.write_all(&body_len.to_le_bytes()).await?;
        writer.seek(SeekFrom::Start(body_end)).await?;
        Ok(())
    }
}

/// A blob being read: the stream, the length its framing must add up to, the
/// digest it has to be stored under to serve, and the codecs its custom values
/// decode with.
#[derive(Debug)]
pub(super) struct BlobReader<'a, R> {
    pub(super) reader: &'a mut R,
    pub(super) file_len: u64,
    pub(super) digest: Digest,
    pub(super) codecs: &'a Codecs,
}

impl<R> BlobReader<'_, R>
where
    R: AsyncRead + Unpin + Send,
{
    pub(super) async fn covers_outputs(&mut self, outputs: &[DynamicValue]) -> FormatResult<bool> {
        let prefix = self
            .scan_header(|idx, descriptor| {
                outputs
                    .get(idx)
                    .is_some_and(|value| descriptor_covers(descriptor, value))
            })
            .await?;
        Ok(prefix.is_some_and(|prefix| prefix.output_count == outputs.len()))
    }

    /// Whether this blob can serve `demand` under its digest: [`read`](Self::read) stopped
    /// after its header check. Reads the fixed prefix plus one descriptor per output and stops,
    /// so a reuse verdict costs one small sequential read instead of decoding the body. Sharing
    /// `read_header` is what keeps a probe's verdict and the later read's from drifting apart.
    pub(super) async fn covers_demand(&mut self, demand: &[OutputDemand]) -> FormatResult<bool> {
        Ok(self.read_header(demand).await?.is_some())
    }

    pub(super) async fn read(
        &mut self,
        demand: &[OutputDemand],
    ) -> FormatResult<Option<Vec<DynamicValue>>> {
        let Some(header) = self.read_header(demand).await? else {
            return Ok(None);
        };
        let mut values = Vec::with_capacity(header.descriptors.len());
        for descriptor in header.descriptors {
            let value = match descriptor.kind {
                OutputKind::Unbound => DynamicValue::Unbound,
                OutputKind::Static => {
                    let mut payload = (&mut *self.reader).take(descriptor.payload_len);
                    let value = read_static(&mut payload, descriptor.payload_len).await?;
                    require_consumed(&payload)?;
                    DynamicValue::Static(value)
                }
                OutputKind::Custom { type_id, .. } => {
                    let codec = self
                        .codecs
                        .get(type_id)
                        .expect("custom codec was validated while reading the header");
                    let mut payload = (&mut *self.reader).take(descriptor.payload_len);
                    let value = codec
                        .decode(&mut payload, descriptor.payload_len)
                        .await
                        .map_err(|source| CodecFormatError::Decode { type_id, source })?;
                    require_consumed(&payload)?;
                    DynamicValue::Custom(value)
                }
            };
            values.push(value);
        }
        Ok(Some(values))
    }

    /// The blob's descriptors, or `None` when it cannot serve `demand` — a different digest, a
    /// different arity, a codec version the library no longer has, or a demanded output the
    /// blob left `Unbound`. Undemanded outputs may be missing; that is what makes a narrow
    /// snapshot reusable by a run that wants less than the one which wrote it.
    async fn read_header(&mut self, demand: &[OutputDemand]) -> FormatResult<Option<BlobHeader>> {
        let mut descriptors = Vec::new();
        let prefix = self
            .scan_header(|idx, descriptor| {
                let Some(output_demand) = demand.get(idx) else {
                    return false;
                };
                if !output_demand.is_skip() && matches!(descriptor.kind, OutputKind::Unbound) {
                    return false;
                }
                descriptors.push(descriptor);
                true
            })
            .await?;
        Ok(prefix
            .filter(|prefix| prefix.output_count == demand.len())
            .map(|_| BlobHeader { descriptors }))
    }

    async fn scan_header(
        &mut self,
        mut accept: impl FnMut(usize, OutputDescriptor) -> bool,
    ) -> FormatResult<Option<HeaderPrefix>> {
        let Some(prefix) = self.read_prefix().await? else {
            return Ok(None);
        };
        let mut payload_sum = 0u64;
        for idx in 0..prefix.output_count {
            let descriptor = read_descriptor(self.reader).await?;
            payload_sum = payload_sum
                .checked_add(descriptor.payload_len)
                .ok_or(FrameError::PayloadLengthOverflow)?;
            if let OutputKind::Custom { type_id, version } = descriptor.kind
                && self
                    .codecs
                    .get(type_id)
                    .is_none_or(|codec| codec.version() != version)
            {
                return Ok(None);
            }
            if !accept(idx, descriptor) {
                return Ok(None);
            }
        }
        if payload_sum != prefix.body_len {
            return Err(FrameError::BodyLengthMismatch.into());
        }
        Ok(Some(prefix))
    }

    async fn read_prefix(&mut self) -> FormatResult<Option<HeaderPrefix>> {
        let mut bytes = [0; FIXED_LEN];
        self.reader.read_exact(&mut bytes).await?;
        if &bytes[..8] != MAGIC {
            return Err(FrameError::WrongMagic.into());
        }
        if u32::from_le_bytes(bytes[8..12].try_into().unwrap()) != FORMAT_VERSION {
            return Err(FrameError::UnsupportedVersion.into());
        }
        let stored_digest = Digest(bytes[12..44].try_into().unwrap());
        let output_count = u32::from_le_bytes(bytes[44..48].try_into().unwrap()) as usize;
        let body_len = u64::from_le_bytes(bytes[48..56].try_into().unwrap());
        let header_len = checked_header_len(output_count)
            .and_then(|header_len| u64::try_from(header_len).ok())
            .ok_or(FrameError::HeaderLengthOverflow)?;
        let expected_file_len = header_len
            .checked_add(body_len)
            .ok_or(FrameError::FileLengthOverflow)?;
        if expected_file_len != self.file_len {
            return Err(FrameError::FileLengthMismatch.into());
        }
        if stored_digest != self.digest {
            return Ok(None);
        }
        Ok(Some(HeaderPrefix {
            output_count,
            body_len,
        }))
    }
}

/// The first custom type among `outputs` that `codecs` cannot encode — the
/// verdict [`BlobWriter::write`] would reach, answered before anything is written.
pub(super) fn unsupported_type(outputs: &[DynamicValue], codecs: &Codecs) -> Option<TypeId> {
    outputs.iter().find_map(|value| match value {
        DynamicValue::Custom(value) if codecs.get(value.type_id()).is_none() => {
            Some(value.type_id())
        }
        _ => None,
    })
}

fn descriptor_for(value: &DynamicValue, codecs: &Codecs) -> FormatResult<OutputDescriptor> {
    let kind = match value {
        DynamicValue::Unbound => OutputKind::Unbound,
        DynamicValue::Static(_) => OutputKind::Static,
        DynamicValue::Custom(value) => {
            let type_id = value.type_id();
            let codec = codecs
                .get(type_id)
                .ok_or(CodecFormatError::UnknownType(type_id))?;
            OutputKind::Custom {
                type_id,
                version: codec.version(),
            }
        }
    };
    Ok(OutputDescriptor {
        kind,
        payload_len: 0,
    })
}

fn descriptor_covers(descriptor: OutputDescriptor, value: &DynamicValue) -> bool {
    match value {
        DynamicValue::Unbound => true,
        DynamicValue::Static(_) => matches!(descriptor.kind, OutputKind::Static),
        DynamicValue::Custom(value) => matches!(
            descriptor.kind,
            OutputKind::Custom { type_id, .. } if type_id == value.type_id()
        ),
    }
}

async fn write_descriptor(
    writer: &mut (impl AsyncWrite + Unpin),
    descriptor: OutputDescriptor,
) -> io::Result<()> {
    let mut bytes = [0; DESCRIPTOR_LEN];
    bytes[0] = descriptor.kind.tag();
    if let OutputKind::Custom { type_id, version } = descriptor.kind {
        bytes[4..20].copy_from_slice(&type_id.as_u128().to_le_bytes());
        bytes[20..24].copy_from_slice(&version.to_le_bytes());
    }
    bytes[24..32].copy_from_slice(&descriptor.payload_len.to_le_bytes());
    writer.write_all(&bytes).await
}

async fn read_descriptor(reader: &mut (impl AsyncRead + Unpin)) -> FormatResult<OutputDescriptor> {
    let mut bytes = [0; DESCRIPTOR_LEN];
    reader.read_exact(&mut bytes).await?;
    if bytes[1..4] != [0; 3] {
        return Err(FrameError::ReservedBytes.into());
    }
    let type_id_bytes: [u8; 16] = bytes[4..20].try_into().unwrap();
    let version = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
    let payload_len = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
    let kind = match bytes[0] {
        0 => {
            if type_id_bytes != [0; 16] || version != 0 || payload_len != 0 {
                return Err(FrameError::UnboundMetadata.into());
            }
            OutputKind::Unbound
        }
        1 => {
            if type_id_bytes != [0; 16] || version != 0 {
                return Err(FrameError::StaticMetadata.into());
            }
            OutputKind::Static
        }
        2 => OutputKind::Custom {
            type_id: TypeId::from_u128(u128::from_le_bytes(type_id_bytes)),
            version,
        },
        _ => return Err(FrameError::UnknownValueTag.into()),
    };
    Ok(OutputDescriptor { kind, payload_len })
}

async fn write_static(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &ConstValue,
) -> io::Result<()> {
    match value {
        ConstValue::Null => writer.write_all(&[0]).await,
        ConstValue::Float(value) => {
            writer.write_all(&[1]).await?;
            writer.write_all(&value.to_bits().to_le_bytes()).await
        }
        ConstValue::Int(value) => {
            writer.write_all(&[2]).await?;
            writer.write_all(&value.to_le_bytes()).await
        }
        ConstValue::Bool(value) => writer.write_all(&[3, u8::from(*value)]).await,
        ConstValue::String(value) => write_string(writer, 4, value).await,
        ConstValue::FsPath(path) => write_string(writer, 5, path).await,
        ConstValue::FsPaths(paths) => write_strings(writer, 6, paths).await,
        ConstValue::Enum(value) => write_string(writer, 7, value).await,
    }
}

async fn write_string(
    writer: &mut (impl AsyncWrite + Unpin),
    tag: u8,
    value: &str,
) -> io::Result<()> {
    writer.write_all(&[tag]).await?;
    writer
        .write_all(&(value.len() as u64).to_le_bytes())
        .await?;
    writer.write_all(value.as_bytes()).await
}

async fn write_strings(
    writer: &mut (impl AsyncWrite + Unpin),
    tag: u8,
    values: &[String],
) -> io::Result<()> {
    writer.write_all(&[tag]).await?;
    writer
        .write_all(&(values.len() as u64).to_le_bytes())
        .await?;
    for value in values {
        writer
            .write_all(&(value.len() as u64).to_le_bytes())
            .await?;
        writer.write_all(value.as_bytes()).await?;
    }
    Ok(())
}

async fn read_static(
    reader: &mut (impl AsyncRead + Unpin),
    payload_len: u64,
) -> FormatResult<ConstValue> {
    let tag = read_u8(reader).await?;
    match tag {
        0 => {
            require_payload_len(payload_len, 1)?;
            Ok(ConstValue::Null)
        }
        1 => {
            require_payload_len(payload_len, 9)?;
            Ok(ConstValue::Float(f64::from_bits(read_u64(reader).await?)))
        }
        2 => {
            require_payload_len(payload_len, 9)?;
            Ok(ConstValue::Int(i64::from_le_bytes(
                read_array(reader).await?,
            )))
        }
        3 => {
            require_payload_len(payload_len, 2)?;
            match read_u8(reader).await? {
                0 => Ok(ConstValue::Bool(false)),
                1 => Ok(ConstValue::Bool(true)),
                _ => Err(FrameError::InvalidBool.into()),
            }
        }
        4 => Ok(ConstValue::String(read_string(reader, payload_len).await?)),
        5 => Ok(ConstValue::FsPath(read_string(reader, payload_len).await?)),
        6 => Ok(ConstValue::FsPaths(
            read_strings(reader, payload_len).await?,
        )),
        7 => Ok(ConstValue::Enum(read_string(reader, payload_len).await?)),
        _ => Err(FrameError::UnknownConstVariant.into()),
    }
}

async fn read_string(
    reader: &mut (impl AsyncRead + Unpin),
    payload_len: u64,
) -> FormatResult<String> {
    let byte_len = read_u64(reader).await?;
    let expected_len = 1u64
        .checked_add(8)
        .and_then(|prefix| prefix.checked_add(byte_len))
        .ok_or(FrameError::StringLengthOverflow)?;
    require_payload_len(payload_len, expected_len)?;
    let byte_len = usize::try_from(byte_len).map_err(|_| FrameError::StringTooLarge)?;
    let mut bytes = vec![0; byte_len];
    reader.read_exact(&mut bytes).await?;
    Ok(String::from_utf8(bytes).map_err(FrameError::InvalidUtf8)?)
}

async fn read_strings(
    reader: &mut (impl AsyncRead + Unpin),
    payload_len: u64,
) -> FormatResult<Vec<String>> {
    let value_count = read_u64(reader).await?;
    let minimum_len = 1u64
        .checked_add(8)
        .and_then(|prefix| prefix.checked_add(value_count.checked_mul(8)?))
        .ok_or(FrameError::PathListLengthOverflow)?;
    if minimum_len > payload_len {
        return Err(FrameError::PathCountExceedsPayload.into());
    }
    let value_count = usize::try_from(value_count).map_err(|_| FrameError::PathCountTooLarge)?;
    let mut expected_len = minimum_len;
    let mut values = Vec::with_capacity(value_count);
    for _ in 0..value_count {
        let byte_len = read_u64(reader).await?;
        expected_len = expected_len
            .checked_add(byte_len)
            .ok_or(FrameError::PathListLengthOverflow)?;
        if expected_len > payload_len {
            return Err(FrameError::PathLengthExceedsPayload.into());
        }
        let byte_len = usize::try_from(byte_len).map_err(|_| FrameError::PathTooLarge)?;
        let mut bytes = vec![0; byte_len];
        reader.read_exact(&mut bytes).await?;
        values.push(String::from_utf8(bytes).map_err(FrameError::InvalidUtf8)?);
    }
    require_payload_len(payload_len, expected_len)?;
    Ok(values)
}

async fn read_u8(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<u8> {
    Ok(read_array::<1>(reader).await?[0])
}

async fn read_u64(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<u64> {
    Ok(u64::from_le_bytes(read_array(reader).await?))
}

async fn read_array<const N: usize>(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    reader.read_exact(&mut bytes).await?;
    Ok(bytes)
}

fn require_consumed<R: AsyncRead>(reader: &Take<R>) -> FormatResult<()> {
    if reader.limit() == 0 {
        Ok(())
    } else {
        Err(FrameError::PayloadNotConsumed.into())
    }
}

fn require_payload_len(actual: u64, expected: u64) -> FormatResult<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(FrameError::PayloadLength { actual, expected }.into())
    }
}

pub(super) fn header_len(output_count: usize) -> usize {
    checked_header_len(output_count).expect("a cache header length must fit in memory")
}

fn checked_header_len(output_count: usize) -> Option<usize> {
    FIXED_LEN.checked_add(output_count.checked_mul(DESCRIPTOR_LEN)?)
}

fn descriptor_payload_len_offset(idx: usize) -> u64 {
    u64::try_from(
        FIXED_LEN
            .checked_add(
                idx.checked_mul(DESCRIPTOR_LEN)
                    .expect("a cache descriptor offset must fit in memory"),
            )
            .expect("a cache descriptor offset must fit in memory"),
    )
    .expect("a cache descriptor offset must fit in the file format")
        + PAYLOAD_LEN_OFFSET
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::execution::cache::disk_store::format::FORMAT_VERSION;

    /// Rewrite `blob`'s format version to the one before the current: the header a blob from an
    /// earlier build carries, which every read refuses.
    pub(crate) fn age_format(blob: &mut [u8]) {
        let older = FORMAT_VERSION - 1;
        blob[8..12].copy_from_slice(&older.to_le_bytes());
    }
}

#[cfg(test)]
mod tests;
