//! Codec layer for a node's output values (`Vec<DynamicValue>`) — turns them
//! to/from bytes for the cache. The bytes are stored by
//! [`DiskStore`](crate::execution::disk_store::DiskStore); this module is the
//! value↔bytes half, not the storage.
//!
//! [`DynamicValue`] is deliberately not `Serialize`: `Unbound`/`Static` are
//! trivially serializable, but `Custom(Arc<dyn CustomValue>)` is an opaque
//! runtime payload. Each custom *type* registers a [`CustomValueCodec`] on its
//! entry in the [`Library`](crate::library::Library) type table, and that single
//! entry drives both directions: encode (you have the value — async +
//! context-aware, mirroring preview generation, so a GPU-resident value can read
//! back) and decode (you have only bytes + a type id, since on reload there is no
//! value yet — which is exactly why the registry must exist). A type with no
//! registered codec is left out of the cache, and
//! caching is all-or-nothing per node so a reload never yields a half-real output
//! set. The [`DiskStore`](crate::execution::disk_store::DiskStore) is the one
//! consumer. See `execution/README.md` Part B.

use std::sync::Arc;

use common::{SerdeFormat, deserialize, serialize};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::library::Library;
use crate::runtime::context::ContextManager;
use crate::{CustomValue, DynamicValue, StaticValue, TypeId};

pub type CodecError = Box<dyn std::error::Error + Send + Sync>;

#[async_trait::async_trait]
pub trait CustomValueCodec: Send + Sync + std::fmt::Debug {
    async fn encode(
        &self,
        value: &dyn CustomValue,
        ctx: &mut ContextManager,
    ) -> std::result::Result<Vec<u8>, CodecError>;

    fn decode(&self, bytes: Vec<u8>) -> std::result::Result<Arc<dyn CustomValue>, CodecError>;
}

/// Failure encoding outputs to, or rebuilding them from, the cache. Each variant
/// is an *expected* condition (a GPU readback error, or a blob that outlived the
/// binary that wrote it) — never a logic bug in the caller. Crate-internal: the
/// public codec trait surfaces [`CodecError`], not this.
#[derive(Debug, Error)]
pub(crate) enum Error {
    /// The serialized output frame didn't decode — corrupt, truncated, or
    /// written by an incompatible codec version.
    #[error("malformed cached output frame: {0}")]
    Frame(String),
    /// A registered codec failed to encode a value (e.g. a GPU readback error).
    #[error("encoding a {type_id:?} value failed: {source}")]
    Encode { type_id: TypeId, source: CodecError },
    /// A custom value names a type with no codec registered in this process (the
    /// producing crate isn't loaded, or never registered it). On encode it means
    /// the node simply isn't cacheable; on decode, a written blob can't be rebuilt.
    #[error("no cache codec registered for custom type {0:?}")]
    UnknownType(TypeId),
    /// A registered codec rejected its blob.
    #[error("decoding a {type_id:?} value failed: {source}")]
    Decoder { type_id: TypeId, source: CodecError },
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

/// Version of the cache *blob* framing (the [`CachedValue`] mirror serialized below),
/// prefixed to every blob as 4 little-endian bytes. Bitcode isn't self-describing, so a
/// change to `CachedValue`'s shape would silently mis-decode old blobs; bumping this
/// makes [`deserialize_outputs`] reject them as a miss (→ recompute) instead. It does
/// **not** cover a custom codec's opaque `blob` bytes — that's the codec's own
/// responsibility (see [`CustomValueCodec::decode`]).
const FORMAT_VERSION: u32 = 1;

/// Serializable mirror of one [`DynamicValue`]. `Custom` carries the producer's
/// type id so the loader can pick the right codec.
#[derive(Serialize, Deserialize)]
enum CachedValue {
    Unbound,
    Static(StaticValue),
    Custom { type_id: TypeId, blob: Vec<u8> },
}

/// Encode a node's outputs for the cache. [`Error::UnknownType`] means the node
/// isn't cacheable — a custom output's type has no registered codec; caching is
/// all-or-nothing per node, so the caller skips the whole node (not a failure).
/// [`Error::Encode`] is a real encode failure (e.g. a GPU readback error).
pub(crate) async fn serialize_outputs(
    outputs: &[DynamicValue],
    library: &Library,
    ctx: &mut ContextManager,
) -> Result<Vec<u8>> {
    let mut cached = Vec::with_capacity(outputs.len());
    for value in outputs {
        cached.push(match value {
            DynamicValue::Unbound => CachedValue::Unbound,
            DynamicValue::Static(value) => CachedValue::Static(value.clone()),
            DynamicValue::Custom(value) => {
                let type_id = value.type_id();
                let codec = library.codec(&type_id).ok_or(Error::UnknownType(type_id))?;
                let blob = codec
                    .encode(value.as_ref(), ctx)
                    .await
                    .map_err(|source| Error::Encode { type_id, source })?;
                CachedValue::Custom { type_id, blob }
            }
        });
    }
    // In-memory encode of known-serializable types: failure is a logic bug.
    let body = serialize(&cached, SerdeFormat::Bitcode).expect("cache output serialization");
    let mut bytes = Vec::with_capacity(4 + body.len());
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&body);
    Ok(bytes)
}

/// Decode outputs previously written by [`serialize_outputs`], rebuilding custom
/// values through `registry`. Errors on malformed bytes or an unregistered type.
pub(crate) fn deserialize_outputs(bytes: &[u8], library: &Library) -> Result<Vec<DynamicValue>> {
    if bytes.len() < 4 {
        return Err(Error::Frame(
            "cache blob too short for a version header".into(),
        ));
    }
    let (header, body) = bytes.split_at(4);
    let version = u32::from_le_bytes(header.try_into().expect("4-byte header"));
    if version != FORMAT_VERSION {
        return Err(Error::Frame(format!(
            "unsupported cache format version {version} (expected {FORMAT_VERSION})"
        )));
    }
    let cached: Vec<CachedValue> =
        deserialize(body, SerdeFormat::Bitcode).map_err(|e| Error::Frame(e.to_string()))?;
    cached
        .into_iter()
        .map(|value| {
            Ok(match value {
                CachedValue::Unbound => DynamicValue::Unbound,
                CachedValue::Static(value) => DynamicValue::Static(value),
                CachedValue::Custom { type_id, blob } => {
                    let codec = library.codec(&type_id).ok_or(Error::UnknownType(type_id))?;
                    let value = codec
                        .decode(blob)
                        .map_err(|source| Error::Decoder { type_id, source })?;
                    DynamicValue::Custom(value)
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::TypeEntry;
    use async_trait::async_trait;
    use std::any::Any;
    use std::fmt;
    use std::sync::Arc;

    const BLOB_TYPE: &str = "6c20414f-12a2-4150-bd79-1b4ee23a9f33";
    const OPAQUE_TYPE: &str = "f4a1b423-1ab3-4864-a59d-f4bb2f74ecb1";

    /// A disk-cacheable custom value; its [`BlobCodec`] just moves the bytes.
    #[derive(Debug, PartialEq)]
    struct Blob(Vec<u8>);

    impl fmt::Display for Blob {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Blob({} bytes)", self.0.len())
        }
    }

    impl CustomValue for Blob {
        fn type_id(&self) -> TypeId {
            BLOB_TYPE.into()
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
            self
        }
    }

    #[derive(Debug)]
    struct BlobCodec;

    #[async_trait]
    impl CustomValueCodec for BlobCodec {
        async fn encode(
            &self,
            value: &dyn CustomValue,
            _ctx: &mut ContextManager,
        ) -> std::result::Result<Vec<u8>, CodecError> {
            Ok(value
                .as_any()
                .downcast_ref::<Blob>()
                .expect("BlobCodec is only registered for Blob")
                .0
                .clone())
        }
        fn decode(&self, bytes: Vec<u8>) -> std::result::Result<Arc<dyn CustomValue>, CodecError> {
            Ok(Arc::new(Blob(bytes)))
        }
    }

    /// A custom value used with no codec (skip) or [`FailingCodec`] (error).
    #[derive(Debug)]
    struct Opaque;

    impl fmt::Display for Opaque {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Opaque")
        }
    }

    impl CustomValue for Opaque {
        fn type_id(&self) -> TypeId {
            OPAQUE_TYPE.into()
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
            self
        }
    }

    /// A codec whose `encode` always fails — exercises the `Err` path.
    #[derive(Debug)]
    struct FailingCodec;

    #[async_trait]
    impl CustomValueCodec for FailingCodec {
        async fn encode(
            &self,
            _value: &dyn CustomValue,
            _ctx: &mut ContextManager,
        ) -> std::result::Result<Vec<u8>, CodecError> {
            Err("boom".into())
        }
        fn decode(&self, _bytes: Vec<u8>) -> std::result::Result<Arc<dyn CustomValue>, CodecError> {
            Err("boom".into())
        }
    }

    fn blob_func_lib() -> Library {
        let mut library = Library::default();
        library.register_type(
            BLOB_TYPE,
            TypeEntry::custom_with_codec("Blob", Arc::new(BlobCodec)),
        );
        library
    }

    #[tokio::test]
    async fn static_and_unbound_round_trip() {
        let outputs = vec![
            DynamicValue::Unbound,
            DynamicValue::Static(StaticValue::Int(42)),
            DynamicValue::Static(StaticValue::String("hi".into())),
        ];
        let bytes = serialize_outputs(
            &outputs,
            &Library::default(),
            &mut ContextManager::default(),
        )
        .await
        .expect("all serializable");
        let back = deserialize_outputs(&bytes, &Library::default()).unwrap();

        assert_eq!(back.len(), 3);
        assert!(matches!(back[0], DynamicValue::Unbound));
        assert_eq!(back[1].as_i64(), Some(42));
        assert_eq!(back[2].as_string(), Some("hi"));
    }

    #[tokio::test]
    async fn custom_round_trips_via_registry() {
        let outputs = vec![
            DynamicValue::Static(StaticValue::Bool(true)),
            DynamicValue::from_custom(Blob(vec![1, 2, 3, 255])),
        ];
        let bytes = serialize_outputs(&outputs, &blob_func_lib(), &mut ContextManager::default())
            .await
            .expect("blob is cacheable");
        let back = deserialize_outputs(&bytes, &blob_func_lib()).unwrap();

        assert_eq!(back.len(), 2);
        assert_eq!(back[0].as_bool(), Some(true));
        let blob = back[1].as_custom::<Blob>().expect("rebuilt as Blob");
        assert_eq!(blob, &Blob(vec![1, 2, 3, 255]));
    }

    #[tokio::test]
    async fn unregistered_custom_type_reports_unknown() {
        // No codec for `Opaque` ⇒ the whole output set isn't cacheable (the caller
        // treats `UnknownType` as a skip), even though the `Int` beside it would
        // serialize fine.
        let outputs = vec![
            DynamicValue::Static(StaticValue::Int(1)),
            DynamicValue::from_custom(Opaque),
        ];
        let result = serialize_outputs(
            &outputs,
            &Library::default(),
            &mut ContextManager::default(),
        )
        .await;
        assert!(matches!(result, Err(Error::UnknownType(_))));
    }

    #[tokio::test]
    async fn encode_failure_propagates_as_error() {
        let mut library = Library::default();
        library.register_type(
            OPAQUE_TYPE,
            TypeEntry::custom_with_codec("Opaque", Arc::new(FailingCodec)),
        );
        let outputs = vec![DynamicValue::from_custom(Opaque)];
        let result = serialize_outputs(&outputs, &library, &mut ContextManager::default()).await;
        assert!(matches!(result, Err(Error::Encode { .. })));
    }

    #[tokio::test]
    async fn unregistered_type_errors_on_load() {
        let outputs = vec![DynamicValue::from_custom(Blob(vec![9]))];
        let bytes = serialize_outputs(&outputs, &blob_func_lib(), &mut ContextManager::default())
            .await
            .unwrap();
        // Empty library — the type was written but has no codec here.
        let result = deserialize_outputs(&bytes, &Library::default());
        assert!(matches!(result, Err(Error::UnknownType(_))));
    }

    #[test]
    #[should_panic(expected = "duplicate type registration")]
    fn duplicate_registration_panics() {
        let mut library = Library::default();
        library.register_type(
            BLOB_TYPE,
            TypeEntry::custom_with_codec("Blob", Arc::new(BlobCodec)),
        );
        library.register_type(
            BLOB_TYPE,
            TypeEntry::custom_with_codec("Blob", Arc::new(BlobCodec)),
        );
    }
}
