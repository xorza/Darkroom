//! On-disk codec for a node's output values (`Vec<DynamicValue>`).
//!
//! [`DynamicValue`] is deliberately not `Serialize`: `Unbound`/`Static` are
//! trivially serializable, but `Custom(Arc<dyn CustomValue>)` is an opaque
//! runtime payload. This module bridges that gap for disk-backed caching —
//! `Static`/`Unbound` serialize directly, and each `Custom` value is encoded
//! via [`CustomValue::cache_blob`] and rebuilt on load by a decoder looked up by
//! type id in a [`CustomValueRegistry`]. A value whose `cache_blob` is `None`
//! makes the whole output set non-cacheable, so the caller recomputes rather
//! than persisting a partial result. See `docs/disk-cache-design.md`.

// Consumed by the executor only in the disk-cache integration phase; the
// crate-internal codec helpers are dead until that wiring lands.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use common::{SerdeFormat, deserialize, serialize};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::data::{CustomValue, DynamicValue, StaticValue, TypeId};

/// Rebuilds a custom value from the bytes its [`CustomValue::cache_blob`]
/// produced. Takes the blob **by value** so a decoder can move it straight into
/// the reconstructed value (e.g. an image's pixel buffer) without copying — the
/// caller already owns it and would otherwise drop it. The decoder lives in a
/// downstream crate, so its own failure (bytes that don't match the type's
/// current layout) stays type-erased here; the framework wraps it in
/// [`Error::Decoder`].
pub type CustomDecoder =
    fn(
        Vec<u8>,
    ) -> std::result::Result<Arc<dyn CustomValue>, Box<dyn std::error::Error + Send + Sync>>;

/// Failure rebuilding cached outputs from disk bytes. Each variant is an
/// *expected* condition when a blob outlives the binary or library that wrote
/// it — never a logic bug in the caller.
#[derive(Debug, Error)]
pub enum Error {
    /// The serialized output frame didn't decode — corrupt, truncated, or
    /// written by an incompatible codec version.
    #[error("malformed cached output frame: {0}")]
    Frame(String),
    /// A cached custom value names a type with no decoder registered in this
    /// process (the producing crate isn't loaded, or never registered it).
    #[error("no cache decoder registered for custom type {0:?}")]
    UnknownType(TypeId),
    /// A registered decoder rejected its blob.
    #[error("decoding a {type_id:?} value failed: {source}")]
    Decoder {
        type_id: TypeId,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Maps a custom type's [`TypeId`] to the decoder that rebuilds its values from
/// a cache blob. Downstream crates register the types they want disk-cacheable
/// (`scenarium` itself knows of none); [`deserialize_outputs`] dispatches
/// through it. The encode side needs no registration — it goes through the
/// value's own `cache_blob`.
#[derive(Debug, Default)]
pub struct CustomValueRegistry {
    decoders: HashMap<TypeId, CustomDecoder>,
}

impl CustomValueRegistry {
    /// Register `decoder` as the rebuilder for `type_id`. Panics on a duplicate
    /// registration — two decoders for one type is a wiring bug, not a runtime
    /// condition.
    pub fn register(&mut self, type_id: impl Into<TypeId>, decoder: CustomDecoder) {
        let prev = self.decoders.insert(type_id.into(), decoder);
        assert!(
            prev.is_none(),
            "duplicate custom-value decoder registration"
        );
    }

    fn decode(&self, type_id: &TypeId, bytes: Vec<u8>) -> Result<Arc<dyn CustomValue>> {
        let decoder = self
            .decoders
            .get(type_id)
            .ok_or(Error::UnknownType(*type_id))?;
        decoder(bytes).map_err(|source| Error::Decoder {
            type_id: *type_id,
            source,
        })
    }
}

/// Serializable mirror of one [`DynamicValue`]. `Custom` carries the producer's
/// type id so the loader can pick the right decoder.
#[derive(Serialize, Deserialize)]
enum CachedValue {
    Unbound,
    Static(StaticValue),
    Custom { type_id: TypeId, blob: Vec<u8> },
}

/// Encode a node's outputs for the cache, or `None` if any value can't be
/// persisted (an unbacked `Custom`) — caching is all-or-nothing per node so a
/// reload never yields a half-real output set.
pub(crate) fn serialize_outputs(outputs: &[DynamicValue]) -> Option<Vec<u8>> {
    let mut cached = Vec::with_capacity(outputs.len());
    for value in outputs {
        cached.push(match value {
            DynamicValue::Unbound => CachedValue::Unbound,
            DynamicValue::Static(value) => CachedValue::Static(value.clone()),
            DynamicValue::Custom(value) => CachedValue::Custom {
                type_id: value.type_def().type_id,
                blob: value.cache_blob()?,
            },
        });
    }
    // In-memory encode of known-serializable types: failure is a logic bug.
    Some(serialize(&cached, SerdeFormat::Bitcode).expect("cache output serialization"))
}

/// Decode outputs previously written by [`serialize_outputs`], rebuilding custom
/// values through `registry`. Errors on malformed bytes or an unregistered type
/// — both expected when a cache blob outlives the binary that wrote it.
pub(crate) fn deserialize_outputs(
    bytes: &[u8],
    registry: &CustomValueRegistry,
) -> Result<Vec<DynamicValue>> {
    let cached: Vec<CachedValue> =
        deserialize(bytes, SerdeFormat::Bitcode).map_err(|e| Error::Frame(e.to_string()))?;
    cached
        .into_iter()
        .map(|value| {
            Ok(match value {
                CachedValue::Unbound => DynamicValue::Unbound,
                CachedValue::Static(value) => DynamicValue::Static(value),
                CachedValue::Custom { type_id, blob } => {
                    DynamicValue::Custom(registry.decode(&type_id, blob)?)
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::TypeDef;
    use std::any::Any;
    use std::fmt;

    const BLOB_TYPE: &str = "6c20414f-12a2-4150-bd79-1b4ee23a9f33";
    const OPAQUE_TYPE: &str = "f4a1b423-1ab3-4864-a59d-f4bb2f74ecb1";

    /// A disk-cacheable custom value: its blob is just its bytes.
    #[derive(Debug, PartialEq)]
    struct Blob(Vec<u8>);

    impl fmt::Display for Blob {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Blob({} bytes)", self.0.len())
        }
    }

    impl CustomValue for Blob {
        fn type_def(&self) -> Arc<TypeDef> {
            Arc::new(TypeDef {
                type_id: BLOB_TYPE.into(),
                display_name: "Blob".into(),
            })
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn cache_blob(&self) -> Option<Vec<u8>> {
            Some(self.0.clone())
        }
    }

    fn decode_blob(
        bytes: Vec<u8>,
    ) -> std::result::Result<Arc<dyn CustomValue>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Arc::new(Blob(bytes)))
    }

    /// A custom value with no disk codec (default `cache_blob` → `None`).
    #[derive(Debug)]
    struct Opaque;

    impl fmt::Display for Opaque {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Opaque")
        }
    }

    impl CustomValue for Opaque {
        fn type_def(&self) -> Arc<TypeDef> {
            Arc::new(TypeDef {
                type_id: OPAQUE_TYPE.into(),
                display_name: "Opaque".into(),
            })
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    fn blob_registry() -> CustomValueRegistry {
        let mut registry = CustomValueRegistry::default();
        registry.register(BLOB_TYPE, decode_blob);
        registry
    }

    #[test]
    fn static_and_unbound_round_trip() {
        let outputs = vec![
            DynamicValue::Unbound,
            DynamicValue::Static(StaticValue::Int(42)),
            DynamicValue::Static(StaticValue::String("hi".into())),
        ];
        let bytes = serialize_outputs(&outputs).expect("all serializable");
        let back = deserialize_outputs(&bytes, &CustomValueRegistry::default()).unwrap();

        assert_eq!(back.len(), 3);
        assert!(matches!(back[0], DynamicValue::Unbound));
        assert_eq!(back[1].as_i64(), Some(42));
        assert_eq!(back[2].as_string(), Some("hi"));
    }

    #[test]
    fn custom_round_trips_via_registry() {
        let outputs = vec![
            DynamicValue::Static(StaticValue::Bool(true)),
            DynamicValue::from_custom(Blob(vec![1, 2, 3, 255])),
        ];
        let bytes = serialize_outputs(&outputs).expect("blob is cacheable");
        let back = deserialize_outputs(&bytes, &blob_registry()).unwrap();

        assert_eq!(back.len(), 2);
        assert_eq!(back[0].as_bool(), Some(true));
        let blob = back[1].as_custom::<Blob>().expect("rebuilt as Blob");
        assert_eq!(blob, &Blob(vec![1, 2, 3, 255]));
    }

    #[test]
    fn non_cacheable_custom_makes_whole_set_none() {
        // One unbacked custom value poisons the whole output set, even though
        // the other values would serialize fine.
        let outputs = vec![
            DynamicValue::Static(StaticValue::Int(1)),
            DynamicValue::from_custom(Opaque),
        ];
        assert!(serialize_outputs(&outputs).is_none());
    }

    #[test]
    fn unregistered_type_errors_on_load() {
        let outputs = vec![DynamicValue::from_custom(Blob(vec![9]))];
        let bytes = serialize_outputs(&outputs).unwrap();
        // Empty registry — the type was written but has no decoder here.
        let result = deserialize_outputs(&bytes, &CustomValueRegistry::default());
        assert!(matches!(result, Err(Error::UnknownType(_))));
    }

    #[test]
    #[should_panic(expected = "duplicate custom-value decoder")]
    fn duplicate_registration_panics() {
        let mut registry = CustomValueRegistry::default();
        registry.register(BLOB_TYPE, decode_blob);
        registry.register(BLOB_TYPE, decode_blob);
    }
}
