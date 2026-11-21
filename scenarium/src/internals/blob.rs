//! [`Blob`]: a custom value of raw bytes, and [`BlobCodec`], its cache codec
//! with the knobs the cache tests turn.

use std::fmt;
use std::io::{Read, Write};
use std::sync::Arc;

use crate::data::codec::error::CodecError;
use crate::data::codec::{Codecs, CustomValueCodec};
use crate::data::data_type::custom_type::CustomType;
use crate::data::data_type::custom_type::CustomTypeId;
use crate::data::dynamic_value::{CustomValue, DynamicValue};
use crate::data::ram_usage::RamUsage;
use crate::internals::calls::Calls;
use crate::library::Library;

pub(crate) const BLOB_TYPE: CustomTypeId =
    CustomTypeId::literal("78391861-24da-4368-a3a5-2a6b7a47f112");

/// Bytes on a wire, weighing their length in CPU RAM.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Blob(pub(crate) Vec<u8>);

impl Blob {
    /// `bytes` as a wire value.
    pub(crate) fn value(bytes: impl Into<Vec<u8>>) -> DynamicValue {
        DynamicValue::from_custom(Self(bytes.into()))
    }
}

impl fmt::Display for Blob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Blob({} bytes)", self.0.len())
    }
}

impl CustomValue for Blob {
    fn custom_type_id(&self) -> CustomTypeId {
        BLOB_TYPE
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
    pub(crate) fault: BlobFault,
}

/// The fault a [`BlobCodec`] injects.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlobFault {
    #[default]
    None,
    /// Encoding writes the bytes and then fails.
    EncodeFails,
    /// Decoding reads none of the payload it is handed.
    DecodeUnderReads,
}

impl BlobCodec {
    /// A faultless codec at `version`, counting its decodes in `decodes`.
    pub(crate) fn versioned(version: u32, decodes: Calls) -> Self {
        Self {
            version,
            decodes,
            fault: BlobFault::None,
        }
    }

    /// A library holding [`Blob`] with this codec.
    pub(crate) fn library(self) -> Library {
        let mut library = Library::default();
        library.add_custom_type(BLOB_TYPE, CustomType::with_codec("Blob", Arc::new(self)));
        library
    }

    /// The codecs of [`Self::library`].
    pub(crate) fn codecs(self) -> Codecs {
        Codecs::clone(self.library().codecs())
    }
}

impl CustomValueCodec for BlobCodec {
    fn version(&self) -> u32 {
        self.version
    }

    fn encode(&self, value: &dyn CustomValue, writer: &mut dyn Write) -> Result<(), CodecError> {
        let blob = value
            .downcast_ref::<Blob>()
            .expect("BlobCodec is only registered for Blob");
        writer.write_all(&blob.0)?;
        if self.fault == BlobFault::EncodeFails {
            return Err("injected encode failure".into());
        }
        Ok(())
    }

    fn decode(
        &self,
        reader: &mut dyn Read,
        byte_len: u64,
    ) -> Result<Arc<dyn CustomValue>, CodecError> {
        self.decodes.bump();
        let mut bytes = Vec::with_capacity(usize::try_from(byte_len)?);
        if self.fault != BlobFault::DecodeUnderReads {
            reader.read_to_end(&mut bytes)?;
        }
        Ok(Arc::new(Blob(bytes)))
    }
}
