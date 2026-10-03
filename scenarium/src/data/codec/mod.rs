//! Streaming codec contract for custom runtime values stored in the disk cache.

pub(crate) mod error;

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::data::codec::error::CodecError;
use crate::{CustomValue, TypeId};

#[async_trait::async_trait]
pub trait CustomValueCodec: Send + Sync + Debug {
    /// Version of this codec's persisted representation. Increment it whenever
    /// previously encoded bytes must not be decoded by the current implementation.
    fn version(&self) -> u32;

    async fn encode(
        &self,
        value: &dyn CustomValue,
        writer: &mut (dyn AsyncWrite + Unpin + Send),
    ) -> Result<(), CodecError>;

    async fn decode(
        &self,
        reader: &mut (dyn AsyncRead + Unpin + Send),
        byte_len: u64,
    ) -> Result<Arc<dyn CustomValue>, CodecError>;
}

/// The disk codecs by type: kept by the [`Library`](crate::library::Library) as
/// types register, and shared with every program compiled from it, so cache
/// I/O reads the codecs of the program it serves.
#[derive(Debug, Default, Clone)]
pub(crate) struct Codecs {
    by_type: HashMap<TypeId, Arc<dyn CustomValueCodec>>,
}

impl Codecs {
    pub(crate) fn get(&self, type_id: TypeId) -> Option<&dyn CustomValueCodec> {
        self.by_type.get(&type_id).map(Arc::as_ref)
    }

    pub(crate) fn insert(&mut self, type_id: TypeId, codec: Arc<dyn CustomValueCodec>) {
        self.by_type.insert(type_id, codec);
    }
}
