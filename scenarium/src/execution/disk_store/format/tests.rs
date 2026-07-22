use std::any::Any;
use std::fmt;
use std::io::Cursor;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncSeek, AsyncWrite, AsyncWriteExt as _, ReadBuf};

use crate::execution::codec;
use crate::execution::digest::Digest;
use crate::execution::disk_store::format::{
    BODY_LEN_OFFSET, DESCRIPTOR_LEN, FIXED_LEN, FORMAT_VERSION, MAGIC, PAYLOAD_LEN_OFFSET,
    covers_outputs, header_len, read, write,
};
use crate::library::{Library, TypeEntry};
use crate::runtime::context::ContextManager;
use crate::{CodecError, CustomValue, CustomValueCodec, DynamicValue, StaticValue, TypeId};

static BLOB_TYPE: LazyLock<TypeId> = LazyLock::new(TypeId::unique);

#[derive(Debug)]
struct ChunkedIo<T, const N: usize>(T);

impl<T, const N: usize> AsyncRead for ChunkedIo<T, N>
where
    T: AsyncRead + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if output.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let mut bytes = [0; N];
        let mut limited = ReadBuf::new(&mut bytes[..output.remaining().min(N)]);
        match Pin::new(&mut self.get_mut().0).poll_read(cx, &mut limited) {
            Poll::Ready(Ok(())) => {
                output.put_slice(limited.filled());
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T, const N: usize> AsyncWrite for ChunkedIo<T, N>
where
    T: AsyncWrite + Unpin,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let chunk_len = bytes.len().min(N);
        Pin::new(&mut self.get_mut().0).poll_write(cx, &bytes[..chunk_len])
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_shutdown(cx)
    }
}

impl<T, const N: usize> AsyncSeek for ChunkedIo<T, N>
where
    T: AsyncSeek + Unpin,
{
    fn start_seek(self: Pin<&mut Self>, position: std::io::SeekFrom) -> std::io::Result<()> {
        Pin::new(&mut self.get_mut().0).start_seek(position)
    }

    fn poll_complete(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<u64>> {
        Pin::new(&mut self.get_mut().0).poll_complete(cx)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Blob(Vec<u8>);

impl fmt::Display for Blob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Blob({} bytes)", self.0.len())
    }
}

impl CustomValue for Blob {
    fn type_id(&self) -> TypeId {
        *BLOB_TYPE
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }
}

#[derive(Clone, Copy, Debug)]
enum DecodeBehavior {
    ReadAll,
    ReadNone,
}

#[derive(Debug)]
struct BlobCodec {
    version: u32,
    behavior: DecodeBehavior,
    decode_calls: Arc<AtomicU64>,
}

#[async_trait::async_trait]
impl CustomValueCodec for BlobCodec {
    fn version(&self) -> u32 {
        self.version
    }

    async fn encode(
        &self,
        value: &dyn CustomValue,
        writer: &mut (dyn AsyncWrite + Unpin + Send),
        _ctx: &mut ContextManager,
    ) -> std::result::Result<(), CodecError> {
        let blob = value
            .as_any()
            .downcast_ref::<Blob>()
            .expect("BlobCodec is only registered for Blob");
        writer.write_all(&blob.0).await?;
        Ok(())
    }

    async fn decode(
        &self,
        reader: &mut (dyn AsyncRead + Unpin + Send),
        byte_len: u64,
    ) -> std::result::Result<Arc<dyn CustomValue>, CodecError> {
        self.decode_calls.fetch_add(1, Ordering::SeqCst);
        let mut bytes = Vec::with_capacity(usize::try_from(byte_len)?);
        if matches!(self.behavior, DecodeBehavior::ReadAll) {
            reader.read_to_end(&mut bytes).await?;
        }
        Ok(Arc::new(Blob(bytes)))
    }
}

fn library(version: u32, behavior: DecodeBehavior, decode_calls: Arc<AtomicU64>) -> Library {
    let mut library = Library::default();
    library.register_type(
        *BLOB_TYPE,
        TypeEntry::custom_with_codec(
            "Blob",
            Arc::new(BlobCodec {
                version,
                behavior,
                decode_calls,
            }),
        ),
    );
    library
}

async fn encoded(digest: Digest, outputs: &[DynamicValue], library: &Library) -> Vec<u8> {
    let mut writer = ChunkedIo::<_, 3>(Cursor::new(Vec::new()));
    write(
        &mut writer,
        digest,
        outputs,
        library,
        &mut ContextManager::default(),
    )
    .await
    .unwrap();
    writer.0.into_inner()
}

#[tokio::test]
async fn indexed_header_checks_without_body_and_all_values_round_trip() {
    let calls = Arc::new(AtomicU64::new(0));
    let library = library(7, DecodeBehavior::ReadAll, calls.clone());
    let digest = Digest([3; 32]);
    let first_blob = (0u8..=255)
        .cycle()
        .take(128 * 1024 + 17)
        .collect::<Vec<_>>();
    let second_blob = (0u8..=255)
        .rev()
        .cycle()
        .take(256 * 1024 + 9)
        .collect::<Vec<_>>();
    let outputs = vec![
        DynamicValue::Unbound,
        DynamicValue::Static(StaticValue::Null),
        DynamicValue::Static(StaticValue::Float(f64::from_bits(0x7ff8_0000_0000_0001))),
        DynamicValue::Static(StaticValue::Int(-42)),
        DynamicValue::Static(StaticValue::Bool(true)),
        DynamicValue::Static(StaticValue::String("hello".into())),
        DynamicValue::Static(StaticValue::FsPath("frames/light.fit".into())),
        DynamicValue::Static(StaticValue::Enum("Screen".into())),
        DynamicValue::from_custom(Blob(first_blob.clone())),
        DynamicValue::from_custom(Blob(second_blob.clone())),
        DynamicValue::Static(StaticValue::Int(99)),
    ];
    let bytes = encoded(digest, &outputs, &library).await;
    let header_len = header_len(outputs.len());
    assert_eq!(&bytes[..8], MAGIC);
    assert_eq!(
        u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
        FORMAT_VERSION
    );
    assert_eq!(
        u64::from_le_bytes(bytes[48..56].try_into().unwrap()) as usize,
        bytes.len() - header_len
    );

    let mut header_only = Cursor::new(&bytes[..header_len]);
    assert!(
        covers_outputs(
            &mut header_only,
            bytes.len() as u64,
            digest,
            &outputs,
            &library,
        )
        .await
        .unwrap()
    );
    assert_eq!(header_only.position() as usize, header_len);

    assert!(
        read(
            &mut Cursor::new(&bytes),
            bytes.len() as u64,
            digest,
            &library,
            outputs.len(),
            |index| index == 0,
        )
        .await
        .unwrap()
        .is_none()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let mut reader = ChunkedIo::<_, 2>(Cursor::new(&bytes));
    let restored = read(
        &mut reader,
        bytes.len() as u64,
        digest,
        &library,
        outputs.len(),
        |_| false,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(restored.len(), outputs.len());
    for (actual, expected) in restored.iter().zip(&outputs).take(8) {
        match (actual, expected) {
            (DynamicValue::Unbound, DynamicValue::Unbound) => {}
            (DynamicValue::Static(actual), DynamicValue::Static(expected)) => {
                assert_eq!(actual, expected)
            }
            _ => panic!("restored value kind differs from the encoded value"),
        }
    }
    assert_eq!(
        restored[8].as_custom::<Blob>().unwrap().0.as_slice(),
        first_blob
    );
    assert_eq!(
        restored[9].as_custom::<Blob>().unwrap().0.as_slice(),
        second_blob
    );
    assert_eq!(restored[10].as_i64(), Some(99));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn custom_decoder_is_bounded_and_must_consume_its_payload() {
    let digest = Digest([4; 32]);
    let outputs = vec![
        DynamicValue::from_custom(Blob(vec![10, 11, 12])),
        DynamicValue::Static(StaticValue::Int(77)),
    ];
    let calls = Arc::new(AtomicU64::new(0));
    let complete_library = library(1, DecodeBehavior::ReadAll, calls.clone());
    let bytes = encoded(digest, &outputs, &complete_library).await;
    let restored = read(
        &mut Cursor::new(&bytes),
        bytes.len() as u64,
        digest,
        &complete_library,
        outputs.len(),
        |_| false,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        restored[0].as_custom::<Blob>(),
        Some(&Blob(vec![10, 11, 12]))
    );
    assert_eq!(restored[1].as_i64(), Some(77));

    let underread_calls = Arc::new(AtomicU64::new(0));
    let underread_library = library(1, DecodeBehavior::ReadNone, underread_calls.clone());
    let error = read(
        &mut Cursor::new(&bytes),
        bytes.len() as u64,
        digest,
        &underread_library,
        outputs.len(),
        |_| false,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, codec::Error::Frame(_)));
    assert_eq!(underread_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn descriptors_selectively_validate_codecs_and_coverage() {
    let digest = Digest([5; 32]);
    let calls = Arc::new(AtomicU64::new(0));
    let registered = library(2, DecodeBehavior::ReadAll, calls);
    let outputs = vec![
        DynamicValue::Static(StaticValue::Int(1)),
        DynamicValue::from_custom(Blob(vec![2])),
    ];
    let bytes = encoded(digest, &outputs, &registered).await;

    assert!(
        covers_outputs(
            &mut Cursor::new(&bytes),
            bytes.len() as u64,
            digest,
            &outputs,
            &registered,
        )
        .await
        .unwrap()
    );
    let partial = vec![
        DynamicValue::Static(StaticValue::Int(1)),
        DynamicValue::Unbound,
    ];
    assert!(
        covers_outputs(
            &mut Cursor::new(&bytes),
            bytes.len() as u64,
            digest,
            &partial,
            &registered,
        )
        .await
        .unwrap()
    );
    let wrong_kind = vec![
        DynamicValue::from_custom(Blob(vec![])),
        DynamicValue::from_custom(Blob(vec![2])),
    ];
    assert!(
        !covers_outputs(
            &mut Cursor::new(&bytes),
            bytes.len() as u64,
            digest,
            &wrong_kind,
            &registered,
        )
        .await
        .unwrap()
    );

    let changed_calls = Arc::new(AtomicU64::new(0));
    let changed = library(3, DecodeBehavior::ReadAll, changed_calls.clone());
    assert!(
        !covers_outputs(
            &mut Cursor::new(&bytes),
            bytes.len() as u64,
            digest,
            &outputs,
            &changed,
        )
        .await
        .unwrap()
    );
    assert_eq!(changed_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn malformed_header_lengths_tags_and_static_values_are_rejected() {
    let digest = Digest([6; 32]);
    let outputs = vec![DynamicValue::Static(StaticValue::Bool(true))];
    let library = Library::default();
    let original = encoded(digest, &outputs, &library).await;

    let mut malformed = Vec::new();
    let mut wrong_magic = original.clone();
    wrong_magic[0] ^= 0xff;
    malformed.push(wrong_magic);
    let mut wrong_version = original.clone();
    wrong_version[8..12].copy_from_slice(&FORMAT_VERSION.wrapping_add(1).to_le_bytes());
    malformed.push(wrong_version);
    let mut reserved = original.clone();
    reserved[FIXED_LEN + 1] = 1;
    malformed.push(reserved);
    let mut unknown_tag = original.clone();
    unknown_tag[FIXED_LEN] = 9;
    malformed.push(unknown_tag);
    let mut wrong_payload_len = original.clone();
    let length_offset = FIXED_LEN + PAYLOAD_LEN_OFFSET as usize;
    wrong_payload_len[length_offset..length_offset + 8].copy_from_slice(&3u64.to_le_bytes());
    malformed.push(wrong_payload_len);
    let mut wrong_body_len = original.clone();
    wrong_body_len[BODY_LEN_OFFSET as usize..BODY_LEN_OFFSET as usize + 8]
        .copy_from_slice(&3u64.to_le_bytes());
    malformed.push(wrong_body_len);
    let mut trailing = original.clone();
    trailing.push(0);
    malformed.push(trailing);

    for bytes in malformed {
        assert!(
            covers_outputs(
                &mut Cursor::new(&bytes),
                bytes.len() as u64,
                digest,
                &outputs,
                &library,
            )
            .await
            .is_err()
        );
    }

    let mut invalid_bool = original.clone();
    invalid_bool[header_len(1) + 1] = 2;
    let error = read(
        &mut Cursor::new(&invalid_bool),
        invalid_bool.len() as u64,
        digest,
        &library,
        outputs.len(),
        |_| false,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, codec::Error::Frame(_)));

    assert_eq!(DESCRIPTOR_LEN, 32);
}
