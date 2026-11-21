//! Streaming codec contract for custom runtime values stored in the disk cache.

pub(crate) mod error;

use std::collections::HashMap;
use std::fmt::Debug;
use std::io::{Read, Write};
use std::sync::Arc;

use crate::data::codec::error::CodecError;
use crate::data::data_type::custom_type::CustomTypeId;
use crate::data::dynamic_value::CustomValue;

/// How a custom value type writes itself to the disk cache and reads itself back.
///
/// Synchronous: the cache runs each blob's whole read or write as one closure on the blocking
/// pool, where an async stream would cost a round trip to the pool for every read and write the
/// codec makes.
pub trait CustomValueCodec: Send + Sync + Debug {
    /// Version of this codec's persisted representation. Increment it whenever
    /// previously encoded bytes must not be decoded by the current implementation.
    fn version(&self) -> u32;

    fn encode(&self, value: &dyn CustomValue, writer: &mut dyn Write) -> Result<(), CodecError>;

    /// Read the value back from exactly `byte_len` bytes of `reader`.
    fn decode(
        &self,
        reader: &mut dyn Read,
        byte_len: u64,
    ) -> Result<Arc<dyn CustomValue>, CodecError>;
}

/// The disk codecs by type: kept by the [`Library`](crate::library::Library) as
/// types register, and shared with every program compiled from it, so cache
/// I/O reads the codecs of the program it serves.
#[derive(Debug, Default, Clone)]
pub(crate) struct Codecs {
    by_type: HashMap<CustomTypeId, Arc<dyn CustomValueCodec>>,
}

impl Codecs {
    pub(crate) fn get(&self, type_id: CustomTypeId) -> Option<&dyn CustomValueCodec> {
        self.by_type.get(&type_id).map(Arc::as_ref)
    }
}

impl FromIterator<(CustomTypeId, Arc<dyn CustomValueCodec>)> for Codecs {
    fn from_iter<I: IntoIterator<Item = (CustomTypeId, Arc<dyn CustomValueCodec>)>>(
        codecs: I,
    ) -> Self {
        Self {
            by_type: codecs.into_iter().collect(),
        }
    }
}
