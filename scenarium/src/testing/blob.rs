//! [`Blob`]: a custom value of raw bytes, and [`BlobCodec`], its cache codec
//! with the knobs the cache tests turn.

use std::any::Any;
use std::fmt;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use crate::library::{Library, TypeEntry};
use crate::testing::calls::Calls;
use crate::{CodecError, CustomValue, CustomValueCodec, DynamicValue, RamUsage, TypeId};

pub(crate) const BLOB_TYPE: TypeId = TypeId::literal("78391861-24da-4368-a3a5-2a6b7a47f112");

/// Bytes on a wire, weighing their length in CPU RAM.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Blob(pub(crate) Vec<u8>);

impl Blob {
    /// `bytes` as a wire value.
    pub(crate) fn value(bytes: impl Into<Vec<u8>>) -> DynamicValue {
        DynamicValue::from_custom(Blob(bytes.into()))
    }
}

impl fmt::Display for Blob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Blob({} bytes)", self.0.len())
    }
}

impl CustomValue for Blob {
    fn type_id(&self) -> TypeId {
        BLOB_TYPE
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }

    fn ram_bytes(&self) -> RamUsage {
        RamUsage {
            cpu: self.0.len(),
            gpu: 0,
        }
    }
}

/// [`Blob`]'s codec: the bytes as they are.
#[derive(Debug, Default)]
pub(crate) struct BlobCodec {
    pub(crate) version: u32,
    /// Counts each decode.
    pub(crate) decodes: Calls,
    /// Encoding writes the bytes and then fails.
    pub(crate) fail_encode: bool,
    /// Decoding reads none of the payload it is handed.
    pub(crate) under_read: bool,
}

impl BlobCodec {
    /// A library holding [`Blob`] with this codec.
    pub(crate) fn library(self) -> Library {
        let mut library = Library::default();
        library.register_type(
            BLOB_TYPE,
            TypeEntry::custom_with_codec("Blob", Arc::new(self)),
        );
        library
    }
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
    ) -> Result<(), CodecError> {
        let blob = value
            .as_any()
            .downcast_ref::<Blob>()
            .expect("BlobCodec is only registered for Blob");
        writer.write_all(&blob.0).await?;
        if self.fail_encode {
            return Err("injected encode failure".into());
        }
        Ok(())
    }

    async fn decode(
        &self,
        reader: &mut (dyn AsyncRead + Unpin + Send),
        byte_len: u64,
    ) -> Result<Arc<dyn CustomValue>, CodecError> {
        self.decodes.bump();
        let mut bytes = Vec::with_capacity(usize::try_from(byte_len)?);
        if !self.under_read {
            reader.read_to_end(&mut bytes).await?;
        }
        Ok(Arc::new(Blob(bytes)))
    }
}
