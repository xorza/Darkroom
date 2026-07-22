//! Indexed binary format for streamed node-output cache blobs.

use std::io::{self, SeekFrom};

use tokio::io::{
    AsyncRead, AsyncReadExt as _, AsyncSeek, AsyncSeekExt as _, AsyncWrite, AsyncWriteExt as _,
};

use crate::execution::codec;
use crate::execution::digest::Digest;
use crate::library::Library;
use crate::runtime::context::ContextManager;
use crate::{DynamicValue, StaticValue, TypeId};

const MAGIC: &[u8; 8] = b"SCENBLOB";
const FORMAT_VERSION: u32 = 7;
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
    fn tag(self) -> u8 {
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

pub(crate) async fn write<W>(
    writer: &mut W,
    digest: Digest,
    outputs: &[DynamicValue],
    library: &Library,
    ctx: &mut ContextManager,
) -> codec::Result<()>
where
    W: AsyncWrite + AsyncSeek + Unpin + Send,
{
    let output_count =
        u32::try_from(outputs.len()).expect("a node output count must fit in the cache format");
    let mut fixed = [0; FIXED_LEN];
    fixed[..8].copy_from_slice(MAGIC);
    fixed[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    fixed[12..44].copy_from_slice(&digest.0);
    fixed[44..48].copy_from_slice(&output_count.to_le_bytes());
    writer.write_all(&fixed).await?;

    for value in outputs {
        write_descriptor(writer, descriptor_for(value, library)?).await?;
    }

    let body_start = u64::try_from(header_len(outputs.len()))
        .expect("a cache header length must fit in the file format");
    debug_assert_eq!(writer.stream_position().await?, body_start);

    for (index, value) in outputs.iter().enumerate() {
        if matches!(value, DynamicValue::Unbound) {
            continue;
        }
        let payload_start = writer.stream_position().await?;
        match value {
            DynamicValue::Unbound => unreachable!(),
            DynamicValue::Static(value) => write_static(writer, value).await?,
            DynamicValue::Custom(value) => {
                let type_id = value.type_id();
                let codec = library
                    .codec(&type_id)
                    .expect("custom output codec was checked while writing descriptors");
                codec
                    .encode(value.as_ref(), writer, ctx)
                    .await
                    .map_err(|source| codec::Error::Encode { type_id, source })?;
            }
        }
        let payload_end = writer.stream_position().await?;
        let payload_len = payload_end
            .checked_sub(payload_start)
            .expect("a streaming cache writer cannot move before its payload start");
        writer
            .seek(SeekFrom::Start(descriptor_payload_len_offset(index)))
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

pub(crate) async fn covers_outputs<R>(
    reader: &mut R,
    file_len: u64,
    digest: Digest,
    outputs: &[DynamicValue],
    library: &Library,
) -> codec::Result<bool>
where
    R: AsyncRead + Unpin + Send,
{
    let prefix = scan_header(reader, file_len, digest, library, |index, descriptor| {
        outputs
            .get(index)
            .is_some_and(|value| descriptor_covers(descriptor, value))
    })
    .await?;
    Ok(prefix.is_some_and(|prefix| prefix.output_count == outputs.len()))
}

pub(crate) async fn read<R>(
    reader: &mut R,
    file_len: u64,
    digest: Digest,
    library: &Library,
    expected_output_count: usize,
    mut output_required: impl FnMut(usize) -> bool,
) -> codec::Result<Option<Vec<DynamicValue>>>
where
    R: AsyncRead + Unpin + Send,
{
    let Some(header) = read_header(
        reader,
        file_len,
        digest,
        library,
        expected_output_count,
        &mut output_required,
    )
    .await?
    else {
        return Ok(None);
    };
    let mut values = Vec::with_capacity(header.descriptors.len());
    for descriptor in header.descriptors {
        let value = match descriptor.kind {
            OutputKind::Unbound => DynamicValue::Unbound,
            OutputKind::Static => {
                let mut payload = (&mut *reader).take(descriptor.payload_len);
                let value = read_static(&mut payload, descriptor.payload_len).await?;
                require_consumed(&payload)?;
                DynamicValue::Static(value)
            }
            OutputKind::Custom { type_id, .. } => {
                let codec = library
                    .codec(&type_id)
                    .expect("custom codec was validated while reading the header");
                let mut payload = (&mut *reader).take(descriptor.payload_len);
                let value = codec
                    .decode(&mut payload, descriptor.payload_len)
                    .await
                    .map_err(|source| codec::Error::Decode { type_id, source })?;
                require_consumed(&payload)?;
                DynamicValue::Custom(value)
            }
        };
        values.push(value);
    }
    Ok(Some(values))
}

async fn read_header<R>(
    reader: &mut R,
    file_len: u64,
    digest: Digest,
    library: &Library,
    expected_output_count: usize,
    output_required: &mut impl FnMut(usize) -> bool,
) -> codec::Result<Option<BlobHeader>>
where
    R: AsyncRead + Unpin + Send,
{
    let mut descriptors = Vec::new();
    let prefix = scan_header(reader, file_len, digest, library, |index, descriptor| {
        if index >= expected_output_count
            || output_required(index) && matches!(descriptor.kind, OutputKind::Unbound)
        {
            return false;
        }
        descriptors.push(descriptor);
        true
    })
    .await?;
    Ok(prefix
        .filter(|prefix| prefix.output_count == expected_output_count)
        .map(|_| BlobHeader { descriptors }))
}

async fn scan_header<R>(
    reader: &mut R,
    file_len: u64,
    digest: Digest,
    library: &Library,
    mut accept: impl FnMut(usize, OutputDescriptor) -> bool,
) -> io::Result<Option<HeaderPrefix>>
where
    R: AsyncRead + Unpin,
{
    let Some(prefix) = read_prefix(reader, file_len, digest).await? else {
        return Ok(None);
    };
    let mut payload_sum = 0u64;
    for index in 0..prefix.output_count {
        let descriptor = read_descriptor(reader).await?;
        payload_sum = payload_sum
            .checked_add(descriptor.payload_len)
            .ok_or_else(|| invalid_data("cache payload lengths overflow u64"))?;
        if let OutputKind::Custom { type_id, version } = descriptor.kind
            && !library
                .codec(&type_id)
                .is_some_and(|codec| codec.version() == version)
        {
            return Ok(None);
        }
        if !accept(index, descriptor) {
            return Ok(None);
        }
    }
    if payload_sum != prefix.body_len {
        return Err(invalid_data(
            "cache descriptor lengths do not equal the declared body length",
        ));
    }
    Ok(Some(prefix))
}

async fn read_prefix<R>(
    reader: &mut R,
    file_len: u64,
    digest: Digest,
) -> io::Result<Option<HeaderPrefix>>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = [0; FIXED_LEN];
    reader.read_exact(&mut bytes).await?;
    if &bytes[..8] != MAGIC {
        return Err(invalid_data("cache header has the wrong magic"));
    }
    if u32::from_le_bytes(bytes[8..12].try_into().unwrap()) != FORMAT_VERSION {
        return Err(invalid_data("cache header has an unsupported version"));
    }
    let stored_digest = Digest(bytes[12..44].try_into().unwrap());
    let output_count = u32::from_le_bytes(bytes[44..48].try_into().unwrap()) as usize;
    let body_len = u64::from_le_bytes(bytes[48..56].try_into().unwrap());
    let header_len = checked_header_len(output_count)
        .ok_or_else(|| invalid_data("cache header length overflow"))?;
    let header_len = u64::try_from(header_len)
        .map_err(|_| invalid_data("cache header length does not fit in the file format"))?;
    let expected_file_len = header_len
        .checked_add(body_len)
        .ok_or_else(|| invalid_data("cache file length overflow"))?;
    if expected_file_len != file_len {
        return Err(invalid_data(
            "cache header and body lengths do not equal the file length",
        ));
    }
    if stored_digest != digest {
        return Ok(None);
    }
    Ok(Some(HeaderPrefix {
        output_count,
        body_len,
    }))
}

fn descriptor_for(value: &DynamicValue, library: &Library) -> codec::Result<OutputDescriptor> {
    let kind = match value {
        DynamicValue::Unbound => OutputKind::Unbound,
        DynamicValue::Static(_) => OutputKind::Static,
        DynamicValue::Custom(value) => {
            let type_id = value.type_id();
            let codec = library
                .codec(&type_id)
                .ok_or(codec::Error::UnknownType(type_id))?;
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

async fn read_descriptor(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<OutputDescriptor> {
    let mut bytes = [0; DESCRIPTOR_LEN];
    reader.read_exact(&mut bytes).await?;
    if bytes[1..4] != [0; 3] {
        return Err(invalid_data("cache descriptor reserved bytes are not zero"));
    }
    let type_id_bytes: [u8; 16] = bytes[4..20].try_into().unwrap();
    let version = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
    let payload_len = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
    let kind = match bytes[0] {
        0 => {
            if type_id_bytes != [0; 16] || version != 0 || payload_len != 0 {
                return Err(invalid_data("unbound cache descriptor carries metadata"));
            }
            OutputKind::Unbound
        }
        1 => {
            if type_id_bytes != [0; 16] || version != 0 {
                return Err(invalid_data(
                    "static cache descriptor carries codec metadata",
                ));
            }
            OutputKind::Static
        }
        2 => OutputKind::Custom {
            type_id: TypeId::from_u128(u128::from_le_bytes(type_id_bytes)),
            version,
        },
        _ => return Err(invalid_data("cache descriptor has an unknown value tag")),
    };
    Ok(OutputDescriptor { kind, payload_len })
}

async fn write_static(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &StaticValue,
) -> io::Result<()> {
    match value {
        StaticValue::Null => writer.write_all(&[0]).await,
        StaticValue::Float(value) => {
            writer.write_all(&[1]).await?;
            writer.write_all(&value.to_bits().to_le_bytes()).await
        }
        StaticValue::Int(value) => {
            writer.write_all(&[2]).await?;
            writer.write_all(&value.to_le_bytes()).await
        }
        StaticValue::Bool(value) => writer.write_all(&[3, u8::from(*value)]).await,
        StaticValue::String(value) => write_string(writer, 4, value).await,
        StaticValue::FsPath(value) => write_string(writer, 5, value).await,
        StaticValue::Enum(value) => write_string(writer, 6, value).await,
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

async fn read_static(
    reader: &mut (impl AsyncRead + Unpin),
    payload_len: u64,
) -> codec::Result<StaticValue> {
    let tag = read_u8(reader).await?;
    match tag {
        0 => {
            require_payload_len(payload_len, 1)?;
            Ok(StaticValue::Null)
        }
        1 => {
            require_payload_len(payload_len, 9)?;
            Ok(StaticValue::Float(f64::from_bits(read_u64(reader).await?)))
        }
        2 => {
            require_payload_len(payload_len, 9)?;
            Ok(StaticValue::Int(i64::from_le_bytes(
                read_array(reader).await?,
            )))
        }
        3 => {
            require_payload_len(payload_len, 2)?;
            match read_u8(reader).await? {
                0 => Ok(StaticValue::Bool(false)),
                1 => Ok(StaticValue::Bool(true)),
                _ => Err(codec::Error::Frame(
                    "cached boolean is not encoded as zero or one".into(),
                )),
            }
        }
        4 => Ok(StaticValue::String(read_string(reader, payload_len).await?)),
        5 => Ok(StaticValue::FsPath(read_string(reader, payload_len).await?)),
        6 => Ok(StaticValue::Enum(read_string(reader, payload_len).await?)),
        _ => Err(codec::Error::Frame(
            "cached static value has an unknown variant".into(),
        )),
    }
}

async fn read_string(
    reader: &mut (impl AsyncRead + Unpin),
    payload_len: u64,
) -> codec::Result<String> {
    let byte_len = read_u64(reader).await?;
    let expected_len = 1u64
        .checked_add(8)
        .and_then(|prefix| prefix.checked_add(byte_len))
        .ok_or_else(|| codec::Error::Frame("cached string length overflow".into()))?;
    require_payload_len(payload_len, expected_len)?;
    let byte_len = usize::try_from(byte_len)
        .map_err(|_| codec::Error::Frame("cached string does not fit in memory".into()))?;
    let mut bytes = vec![0; byte_len];
    reader.read_exact(&mut bytes).await?;
    String::from_utf8(bytes)
        .map_err(|_| codec::Error::Frame("cached string is not valid UTF-8".into()))
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

fn require_consumed<R: AsyncRead>(reader: &tokio::io::Take<R>) -> codec::Result<()> {
    if reader.limit() == 0 {
        Ok(())
    } else {
        Err(codec::Error::Frame(
            "cached payload decoder did not consume its complete region".into(),
        ))
    }
}

fn require_payload_len(actual: u64, expected: u64) -> codec::Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(codec::Error::Frame(format!(
            "cached static payload has length {actual}, expected {expected}"
        )))
    }
}

fn header_len(output_count: usize) -> usize {
    checked_header_len(output_count).expect("a cache header length must fit in memory")
}

fn checked_header_len(output_count: usize) -> Option<usize> {
    FIXED_LEN.checked_add(output_count.checked_mul(DESCRIPTOR_LEN)?)
}

fn descriptor_payload_len_offset(index: usize) -> u64 {
    u64::try_from(
        FIXED_LEN
            .checked_add(
                index
                    .checked_mul(DESCRIPTOR_LEN)
                    .expect("a cache descriptor offset must fit in memory"),
            )
            .expect("a cache descriptor offset must fit in memory"),
    )
    .expect("a cache descriptor offset must fit in the file format")
        + PAYLOAD_LEN_OFFSET
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests;
