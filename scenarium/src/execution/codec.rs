//! Streaming codec contract for custom runtime values stored in the disk cache.

use std::collections::HashMap;
use std::sync::Arc;

use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::runtime::context::ContextStore;
use crate::{CustomValue, TypeId};

pub type CodecError = Box<dyn std::error::Error + Send + Sync>;

#[async_trait::async_trait]
pub trait CustomValueCodec: Send + Sync + std::fmt::Debug {
    /// Version of this codec's persisted representation. Increment it whenever
    /// previously encoded bytes must not be decoded by the current implementation.
    fn version(&self) -> u32;

    async fn encode(
        &self,
        value: &dyn CustomValue,
        writer: &mut (dyn AsyncWrite + Unpin + Send),
        ctx: &mut ContextStore,
    ) -> std::result::Result<(), CodecError>;

    async fn decode(
        &self,
        reader: &mut (dyn AsyncRead + Unpin + Send),
        byte_len: u64,
        ctx: &mut ContextStore,
    ) -> std::result::Result<Arc<dyn CustomValue>, CodecError>;
}

/// The codec registry the disk store retains: `TypeId → codec`, extracted from
/// the [`Library`](crate::library::Library) at store construction so cache I/O
/// doesn't hold the whole registry (funcs, shared graphs, editor metadata).
#[derive(Debug, Default, Clone)]
pub(crate) struct Codecs {
    pub(crate) by_type: HashMap<TypeId, Arc<dyn CustomValueCodec>>,
}

impl Codecs {
    pub(crate) fn get(&self, type_id: TypeId) -> Option<&dyn CustomValueCodec> {
        self.by_type.get(&type_id).map(Arc::as_ref)
    }
}

#[derive(Debug, Error)]
pub(crate) enum Error {
    #[error("cache I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed cached output frame: {0}")]
    Frame(String),
    #[error("encoding a {type_id:?} value failed: {source}")]
    Encode { type_id: TypeId, source: CodecError },
    #[error("no cache codec registered for custom type {0:?}")]
    UnknownType(TypeId),
    #[error("decoding a {type_id:?} value failed: {source}")]
    Decode { type_id: TypeId, source: CodecError },
}

pub(crate) type Result<T> = std::result::Result<T, Error>;
