//! Indexed binary format for node-output cache blobs: a fixed prefix, one descriptor per
//! output, then the outputs' payloads back to back.
//!
//! Every payload's length and blake3 hash stand in its descriptor, and a read holds the payload
//! to both. A blob is published by a rename with no sync, and `rename` promises only that the name
//! "will be atomically replaced" ([rename(2)](https://man7.org/linux/man-pages/man2/rename.2.html)),
//! not that the data reached the disk, so a blob with unwritten blocks after a power loss would
//! otherwise decode as a valid value: a pixel payload has no redundancy of its own. Refused: a sync
//! before the rename, which still misses a later corruption.
//!
//! The header's length is fixed by the output count, so the writer reserves it, streams each
//! payload once through a counter and a hasher, then seeks back and writes the finished header:
//! one seek a blob. Refused: the header after the body, a layout change to save that seek.

use std::io::{self, Read, Seek, SeekFrom, Take, Write};

use crate::data::codec::Codecs;
use crate::data::const_value::ConstValue;
use crate::data::data_type::custom_type::CustomTypeId;
use crate::data::dynamic_value::DynamicValue;
use crate::decl::func::func_lambda::OutputDemand;
use crate::exec::cache::digest::Digest;
use crate::exec::cache::disk_store::error::{CodecFormatError, FormatResult, FrameError};
use crate::identity::program_id::ProgramId;

const MAGIC: &[u8; 8] = b"SCENBLOB";
const FORMAT_VERSION: u32 = 12;
const PATH_OFFSET: usize = 8 + 4 + 32;
const COUNT_OFFSET: usize = PATH_OFFSET + ProgramId::BYTES_LEN;
const FIXED_LEN: usize = COUNT_OFFSET + 4 + 8;
/// Tag, three reserved bytes, the custom type id and codec version, the payload length, the
/// payload hash.
const DESC_LEN: usize = 1 + 3 + 16 + 4 + 8 + 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputKind {
    Unbound,
    Static,
    Custom { type_id: CustomTypeId, version: u32 },
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
struct OutputDesc {
    kind: OutputKind,
    payload_len: u64,
    /// blake3 of the payload's bytes; of no bytes for an unbound output.
    payload_hash: [u8; 32],
}

#[derive(Clone, Copy, Debug)]
struct HeaderPrefix {
    output_count: usize,
    body_len: u64,
}

#[derive(Debug)]
struct BlobHeader {
    descs: Vec<OutputDesc>,
}

/// What a codec writes a payload through: counts and hashes it, so its length and hash are
/// never read back from the stream.
#[derive(Debug)]
struct PayloadWriter<'a, W> {
    writer: &'a mut W,
    written: u64,
    hasher: blake3::Hasher,
}

impl<'a, W: Write> PayloadWriter<'a, W> {
    fn new(writer: &'a mut W) -> Self {
        Self {
            writer,
            written: 0,
            hasher: blake3::Hasher::new(),
        }
    }

    /// The descriptor of what went through, as `kind`.
    fn finish(self, kind: OutputKind) -> OutputDesc {
        OutputDesc {
            kind,
            payload_len: self.written,
            payload_hash: *self.hasher.finalize().as_bytes(),
        }
    }
}

impl<W: Write> Write for PayloadWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.writer.write(bytes)?;
        self.hasher.update(&bytes[..written]);
        self.written += u64::try_from(written).expect("a write's length fits u64");
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

/// What a codec reads a payload through: at most the payload's length, hashed as it is read.
#[derive(Debug)]
struct PayloadReader<'a, R> {
    reader: Take<&'a mut R>,
    hasher: blake3::Hasher,
}

impl<'a, R: Read> PayloadReader<'a, R> {
    fn new(reader: &'a mut R, payload_len: u64) -> Self {
        Self {
            reader: reader.take(payload_len),
            hasher: blake3::Hasher::new(),
        }
    }

    /// Hold what the decoder made of the payload to the payload's descriptor. What the decoder
    /// left unread is hashed too, so a payload whose bytes changed is named so, whatever the
    /// decoder read of it and whether it failed on them; then the decoder's own failure, then a
    /// decoder that stopped short.
    fn finish<T>(
        mut self,
        decoded: FormatResult<T>,
        desc: OutputDesc,
        output_idx: usize,
    ) -> FormatResult<T> {
        let unread = self.reader.limit();
        io::copy(&mut self, &mut io::sink())?;
        if self.reader.limit() != 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into());
        }
        if *self.hasher.finalize().as_bytes() != desc.payload_hash {
            return Err(FrameError::PayloadHash { output_idx }.into());
        }
        let value = decoded?;
        if unread != 0 {
            return Err(FrameError::PayloadNotConsumed.into());
        }
        Ok(value)
    }
}

impl<R: Read> Read for PayloadReader<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let read = self.reader.read(bytes)?;
        self.hasher.update(&bytes[..read]);
        Ok(read)
    }
}

/// A blob being written: the stream, the program node and the digest it is stored under, and the
/// codecs its custom values encode with.
#[derive(Debug)]
pub(super) struct BlobWriter<'a, W> {
    pub(super) writer: &'a mut W,
    pub(super) program_id: ProgramId,
    pub(super) digest: Digest,
    pub(super) codecs: &'a Codecs,
}

impl<W: Write + Seek> BlobWriter<'_, W> {
    /// Reserve the header, stream every payload, then write the header its descriptors made.
    pub(super) fn write(self, outputs: &[DynamicValue]) -> FormatResult<()> {
        let Self {
            writer,
            program_id,
            digest,
            codecs,
        } = self;
        let output_count =
            u32::try_from(outputs.len()).expect("a node output count must fit in the cache format");
        let header_len = header_len(outputs.len());
        let mut header = vec![0_u8; header_len];
        writer.write_all(&header)?;

        let mut body_len = 0_u64;
        let (prefix, desc_slots) = header.split_at_mut(FIXED_LEN);
        let (desc_slots, _) = desc_slots.as_chunks_mut::<DESC_LEN>();
        for (value, desc_slot) in outputs.iter().zip(desc_slots) {
            let mut payload = PayloadWriter::new(&mut *writer);
            let kind = match value {
                DynamicValue::Unbound => OutputKind::Unbound,
                DynamicValue::Static(value) => {
                    write_static(&mut payload, value)?;
                    OutputKind::Static
                }
                DynamicValue::Custom(value) => {
                    let type_id = value.custom_type_id();
                    let codec = codecs
                        .get(type_id)
                        .ok_or(CodecFormatError::UnknownType(type_id))?;
                    codec
                        .encode(value.as_ref(), &mut payload)
                        .map_err(|source| CodecFormatError::Encode { type_id, source })?;
                    OutputKind::Custom {
                        type_id,
                        version: codec.version(),
                    }
                }
            };
            let desc = payload.finish(kind);
            body_len = body_len
                .checked_add(desc.payload_len)
                .ok_or(FrameError::PayloadLenOverflow)?;
            *desc_slot = desc_bytes(desc);
        }

        prefix[..8].copy_from_slice(MAGIC);
        prefix[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        prefix[12..PATH_OFFSET].copy_from_slice(&digest.0);
        prefix[PATH_OFFSET..COUNT_OFFSET].copy_from_slice(&program_id.to_bytes());
        prefix[COUNT_OFFSET..COUNT_OFFSET + 4].copy_from_slice(&output_count.to_le_bytes());
        prefix[COUNT_OFFSET + 4..].copy_from_slice(&body_len.to_le_bytes());
        writer.seek(SeekFrom::Start(0))?;
        writer.write_all(&header)?;
        Ok(())
    }
}

/// A blob being read: the stream, the length its framing must add up to, the program node and
/// digest it has to be stored under to serve, and the codecs its custom values decode with.
#[derive(Debug)]
pub(super) struct BlobReader<'a, R> {
    pub(super) reader: &'a mut R,
    pub(super) file_len: u64,
    pub(super) program_id: ProgramId,
    pub(super) digest: Digest,
    pub(super) codecs: &'a Codecs,
}

impl<R: Read> BlobReader<'_, R> {
    pub(super) fn covers_outputs(&mut self, outputs: &[DynamicValue]) -> FormatResult<bool> {
        let prefix = self.scan_header(|idx, desc| {
            outputs
                .get(idx)
                .is_some_and(|value| desc_covers(desc, value))
        })?;
        Ok(prefix.is_some_and(|prefix| prefix.output_count == outputs.len()))
    }

    /// Whether this blob can serve `demand` under its digest: [`read`](Self::read) stopped after
    /// its header check. Reads the fixed prefix plus one descriptor per output, so a reuse verdict
    /// costs one small sequential read instead of decoding the body. Sharing `read_header` keeps a
    /// probe's verdict and the later read's from drifting apart.
    pub(super) fn covers_demand(&mut self, demand: &[OutputDemand]) -> FormatResult<bool> {
        Ok(self.read_header(demand)?.is_some())
    }

    pub(super) fn read(
        &mut self,
        demand: &[OutputDemand],
    ) -> FormatResult<Option<Vec<DynamicValue>>> {
        let Some(header) = self.read_header(demand)? else {
            return Ok(None);
        };
        let mut values = Vec::with_capacity(header.descs.len());
        for (output_idx, desc) in header.descs.into_iter().enumerate() {
            let value = match desc.kind {
                OutputKind::Unbound => DynamicValue::Unbound,
                OutputKind::Static => {
                    let mut payload = PayloadReader::new(self.reader, desc.payload_len);
                    let value = read_static(&mut payload, desc.payload_len);
                    DynamicValue::Static(payload.finish(value, desc, output_idx)?)
                }
                OutputKind::Custom { type_id, .. } => {
                    let codec = self
                        .codecs
                        .get(type_id)
                        .expect("custom codec was validated while reading the header");
                    let mut payload = PayloadReader::new(self.reader, desc.payload_len);
                    let value = codec
                        .decode(&mut payload, desc.payload_len)
                        .map_err(|source| CodecFormatError::Decode { type_id, source });
                    DynamicValue::Custom(payload.finish(value, desc, output_idx)?)
                }
            };
            values.push(value);
        }
        Ok(Some(values))
    }

    /// The blob's descriptors, or `None` when it cannot serve `demand` — a different digest, a
    /// different arity, a codec version the library no longer has, or a demanded output the blob
    /// left `Unbound`. Undemanded outputs may be missing, so a narrow snapshot is reusable by a
    /// run that wants less than the one which wrote it.
    fn read_header(&mut self, demand: &[OutputDemand]) -> FormatResult<Option<BlobHeader>> {
        let mut descs = Vec::new();
        let prefix = self.scan_header(|idx, desc| {
            let Some(output_demand) = demand.get(idx) else {
                return false;
            };
            if !output_demand.is_skip() && matches!(desc.kind, OutputKind::Unbound) {
                return false;
            }
            descs.push(desc);
            true
        })?;
        Ok(prefix
            .filter(|prefix| prefix.output_count == demand.len())
            .map(|_| BlobHeader { descs }))
    }

    fn scan_header(
        &mut self,
        mut accept: impl FnMut(usize, OutputDesc) -> bool,
    ) -> FormatResult<Option<HeaderPrefix>> {
        let Some(prefix) = self.read_prefix()? else {
            return Ok(None);
        };
        let mut payload_sum = 0u64;
        for idx in 0..prefix.output_count {
            let desc = read_desc(self.reader)?;
            payload_sum = payload_sum
                .checked_add(desc.payload_len)
                .ok_or(FrameError::PayloadLenOverflow)?;
            if let OutputKind::Custom { type_id, version } = desc.kind
                && self
                    .codecs
                    .get(type_id)
                    .is_none_or(|codec| codec.version() != version)
            {
                return Ok(None);
            }
            if !accept(idx, desc) {
                return Ok(None);
            }
        }
        if payload_sum != prefix.body_len {
            return Err(FrameError::BodyLenMismatch.into());
        }
        Ok(Some(prefix))
    }

    fn read_prefix(&mut self) -> FormatResult<Option<HeaderPrefix>> {
        let mut bytes = [0; FIXED_LEN];
        self.reader.read_exact(&mut bytes)?;
        if &bytes[..8] != MAGIC {
            return Err(FrameError::WrongMagic.into());
        }
        if u32::from_le_bytes(bytes[8..12].try_into().unwrap()) != FORMAT_VERSION {
            return Err(FrameError::UnsupportedVersion.into());
        }
        let stored_digest = Digest(bytes[12..PATH_OFFSET].try_into().unwrap());
        let stored_path = &bytes[PATH_OFFSET..COUNT_OFFSET];
        let output_count =
            u32::from_le_bytes(bytes[COUNT_OFFSET..COUNT_OFFSET + 4].try_into().unwrap()) as usize;
        let body_len = u64::from_le_bytes(bytes[COUNT_OFFSET + 4..FIXED_LEN].try_into().unwrap());
        let header_len = checked_header_len(output_count)
            .and_then(|header_len| u64::try_from(header_len).ok())
            .ok_or(FrameError::HeaderLenOverflow)?;
        let expected_file_len = header_len
            .checked_add(body_len)
            .ok_or(FrameError::FileLenOverflow)?;
        if expected_file_len != self.file_len {
            return Err(FrameError::FileLenMismatch.into());
        }
        if stored_digest != self.digest || stored_path != self.program_id.to_bytes() {
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
pub(super) fn unsupported_type(outputs: &[DynamicValue], codecs: &Codecs) -> Option<CustomTypeId> {
    outputs.iter().find_map(|value| match value {
        DynamicValue::Custom(value) if codecs.get(value.custom_type_id()).is_none() => {
            Some(value.custom_type_id())
        }
        _ => None,
    })
}

fn desc_covers(desc: OutputDesc, value: &DynamicValue) -> bool {
    match value {
        DynamicValue::Unbound => true,
        DynamicValue::Static(_) => matches!(desc.kind, OutputKind::Static),
        DynamicValue::Custom(value) => matches!(
            desc.kind,
            OutputKind::Custom { type_id, .. } if type_id == value.custom_type_id()
        ),
    }
}

fn desc_bytes(desc: OutputDesc) -> [u8; DESC_LEN] {
    let mut bytes = [0; DESC_LEN];
    bytes[0] = desc.kind.tag();
    if let OutputKind::Custom { type_id, version } = desc.kind {
        bytes[4..20].copy_from_slice(&type_id.as_u128().to_le_bytes());
        bytes[20..24].copy_from_slice(&version.to_le_bytes());
    }
    bytes[24..32].copy_from_slice(&desc.payload_len.to_le_bytes());
    bytes[32..64].copy_from_slice(&desc.payload_hash);
    bytes
}

fn read_desc(reader: &mut impl Read) -> FormatResult<OutputDesc> {
    let mut bytes = [0; DESC_LEN];
    reader.read_exact(&mut bytes)?;
    if bytes[1..4] != [0; 3] {
        return Err(FrameError::ReservedBytes.into());
    }
    let type_id_bytes: [u8; 16] = bytes[4..20].try_into().unwrap();
    let version = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
    let payload_len = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
    let payload_hash: [u8; 32] = bytes[32..64].try_into().unwrap();
    let kind = match bytes[0] {
        0 => {
            if type_id_bytes != [0; 16]
                || version != 0
                || payload_len != 0
                || payload_hash != *blake3::hash(&[]).as_bytes()
            {
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
            type_id: CustomTypeId::from_u128(u128::from_le_bytes(type_id_bytes)),
            version,
        },
        _ => return Err(FrameError::UnknownValueTag.into()),
    };
    Ok(OutputDesc {
        kind,
        payload_len,
        payload_hash,
    })
}

fn write_static(writer: &mut impl Write, value: &ConstValue) -> io::Result<()> {
    match value {
        ConstValue::Null => writer.write_all(&[0]),
        ConstValue::Float(value) => {
            writer.write_all(&[1])?;
            writer.write_all(&value.to_bits().to_le_bytes())
        }
        ConstValue::Int(value) => {
            writer.write_all(&[2])?;
            writer.write_all(&value.to_le_bytes())
        }
        ConstValue::Bool(value) => writer.write_all(&[3, u8::from(*value)]),
        ConstValue::String(value) => write_string(writer, 4, value),
        ConstValue::FsPath(path) => write_string(writer, 5, path),
        ConstValue::FsPaths(paths) => write_strings(writer, 6, paths),
        ConstValue::Enum(value) => write_string(writer, 7, value),
    }
}

fn write_string(writer: &mut impl Write, tag: u8, value: &str) -> io::Result<()> {
    writer.write_all(&[tag])?;
    writer.write_all(&(value.len() as u64).to_le_bytes())?;
    writer.write_all(value.as_bytes())
}

fn write_strings(writer: &mut impl Write, tag: u8, values: &[String]) -> io::Result<()> {
    writer.write_all(&[tag])?;
    writer.write_all(&(values.len() as u64).to_le_bytes())?;
    for value in values {
        writer.write_all(&(value.len() as u64).to_le_bytes())?;
        writer.write_all(value.as_bytes())?;
    }
    Ok(())
}

fn read_static(reader: &mut impl Read, payload_len: u64) -> FormatResult<ConstValue> {
    let tag = read_u8(reader)?;
    match tag {
        0 => {
            require_payload_len(payload_len, 1)?;
            Ok(ConstValue::Null)
        }
        1 => {
            require_payload_len(payload_len, 9)?;
            Ok(ConstValue::Float(f64::from_bits(read_u64(reader)?)))
        }
        2 => {
            require_payload_len(payload_len, 9)?;
            Ok(ConstValue::Int(i64::from_le_bytes(read_array(reader)?)))
        }
        3 => {
            require_payload_len(payload_len, 2)?;
            match read_u8(reader)? {
                0 => Ok(ConstValue::Bool(false)),
                1 => Ok(ConstValue::Bool(true)),
                _ => Err(FrameError::InvalidBool.into()),
            }
        }
        4 => Ok(ConstValue::String(read_string(reader, payload_len)?)),
        5 => Ok(ConstValue::FsPath(read_string(reader, payload_len)?)),
        6 => Ok(ConstValue::FsPaths(read_strings(reader, payload_len)?)),
        7 => Ok(ConstValue::Enum(read_string(reader, payload_len)?)),
        _ => Err(FrameError::UnknownConstVariant.into()),
    }
}

fn read_string(reader: &mut impl Read, payload_len: u64) -> FormatResult<String> {
    let byte_len = read_u64(reader)?;
    let expected_len = 1u64
        .checked_add(8)
        .and_then(|prefix| prefix.checked_add(byte_len))
        .ok_or(FrameError::StringLenOverflow)?;
    require_payload_len(payload_len, expected_len)?;
    let byte_len = usize::try_from(byte_len).map_err(|_| FrameError::StringTooLarge)?;
    let mut bytes = vec![0; byte_len];
    reader.read_exact(&mut bytes)?;
    Ok(String::from_utf8(bytes).map_err(FrameError::InvalidUtf8)?)
}

fn read_strings(reader: &mut impl Read, payload_len: u64) -> FormatResult<Vec<String>> {
    let value_count = read_u64(reader)?;
    let minimum_len = 1u64
        .checked_add(8)
        .and_then(|prefix| prefix.checked_add(value_count.checked_mul(8)?))
        .ok_or(FrameError::PathListLenOverflow)?;
    if minimum_len > payload_len {
        return Err(FrameError::PathCountExceedsPayload.into());
    }
    let value_count = usize::try_from(value_count).map_err(|_| FrameError::PathCountTooLarge)?;
    let mut expected_len = minimum_len;
    let mut values = Vec::with_capacity(value_count);
    for _ in 0..value_count {
        let byte_len = read_u64(reader)?;
        expected_len = expected_len
            .checked_add(byte_len)
            .ok_or(FrameError::PathListLenOverflow)?;
        if expected_len > payload_len {
            return Err(FrameError::PathLenExceedsPayload.into());
        }
        let byte_len = usize::try_from(byte_len).map_err(|_| FrameError::PathTooLarge)?;
        let mut bytes = vec![0; byte_len];
        reader.read_exact(&mut bytes)?;
        values.push(String::from_utf8(bytes).map_err(FrameError::InvalidUtf8)?);
    }
    require_payload_len(payload_len, expected_len)?;
    Ok(values)
}

fn read_u8(reader: &mut impl Read) -> io::Result<u8> {
    Ok(read_array::<1>(reader)?[0])
}

fn read_u64(reader: &mut impl Read) -> io::Result<u64> {
    Ok(u64::from_le_bytes(read_array(reader)?))
}

fn read_array<const N: usize>(reader: &mut impl Read) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn require_payload_len(actual: u64, expected: u64) -> FormatResult<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(FrameError::PayloadLen { actual, expected }.into())
    }
}

pub(super) fn header_len(output_count: usize) -> usize {
    checked_header_len(output_count).expect("a cache header length must fit in memory")
}

fn checked_header_len(output_count: usize) -> Option<usize> {
    FIXED_LEN.checked_add(output_count.checked_mul(DESC_LEN)?)
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::exec::cache::disk_store::format::FORMAT_VERSION;

    /// Rewrite `blob`'s format version to the one before the current: the header an earlier build
    /// wrote, which every read refuses.
    pub(crate) fn age_format(blob: &mut [u8]) {
        let older = FORMAT_VERSION - 1;
        blob[8..12].copy_from_slice(&older.to_le_bytes());
    }
}

#[cfg(test)]
mod tests;
