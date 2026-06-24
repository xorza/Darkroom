//! Codec registry for serializing a node's custom output values.
//!
//! [`DynamicValue`] is deliberately not `Serialize`: `Unbound`/`Static` are
//! trivially serializable, but `Custom(Arc<dyn CustomValue>)` is an opaque
//! runtime payload. Each custom *type* registers a [`CustomValueCodec`] in a
//! [`CustomValueRegistry`], and that single entry drives both directions: encode
//! (you have the value — async + context-aware, mirroring preview generation, so
//! a GPU-resident value can read back) and decode (you have only bytes + a type
//! id, since on reload there is no value yet — which is exactly why the registry
//! must exist).
//!
//! Dormant: the on-disk output cache that consumed this registry was removed (see
//! git history / `docs/disk-cache-design.md`). The trait + registry are kept so
//! downstream crates (e.g. `lens`) can keep registering codecs for when the disk
//! cache returns; only the registry's `codec` lookup has no caller until then.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::context::ContextManager;
use crate::data::{CustomValue, TypeId};

/// Error a codec hands back to the framework. The codec lives in a downstream
/// crate, so its concrete failure stays type-erased here.
type CodecError = Box<dyn std::error::Error + Send + Sync>;

/// Bidirectional disk codec for one custom-value type, registered once in a
/// [`CustomValueRegistry`]. Encode takes `&dyn CustomValue` (downcast to the
/// codec's concrete type) and is async + context-aware like
/// [`CustomValue::gen_preview`](crate::data::CustomValue::gen_preview), so a
/// GPU-resident value can read back through the [`ContextManager`]. Decode has
/// only bytes — there is no value on reload, which is why dispatch goes through
/// the registry rather than a method on the value.
#[async_trait]
pub trait CustomValueCodec: Send + Sync {
    /// Encode `value` (always this codec's concrete type) for the cache, or
    /// `Err` if encoding failed (e.g. a GPU readback error) — surfaced to the
    /// caller rather than silently dropped. Whether a *type* is cacheable at all
    /// is decided by whether a codec is registered for it, not here.
    async fn encode(
        &self,
        value: &dyn CustomValue,
        ctx: &mut ContextManager,
    ) -> std::result::Result<Vec<u8>, CodecError>;

    /// Rebuild a value from bytes a prior [`Self::encode`] produced. Errors are
    /// expected when a blob outlives the binary that wrote it (corrupt or
    /// layout-changed bytes).
    fn decode(&self, bytes: Vec<u8>) -> std::result::Result<Arc<dyn CustomValue>, CodecError>;
}

/// Maps a custom type's [`TypeId`] to its [`CustomValueCodec`]. Downstream crates
/// register the types they want cacheable (`scenarium` itself knows of none).
#[derive(Default)]
pub struct CustomValueRegistry {
    codecs: HashMap<TypeId, Box<dyn CustomValueCodec>>,
}

impl CustomValueRegistry {
    /// Register `codec` as the encoder/decoder for `type_id`. Panics on a
    /// duplicate registration — two codecs for one type is a wiring bug, not a
    /// runtime condition.
    pub fn register(&mut self, type_id: impl Into<TypeId>, codec: impl CustomValueCodec + 'static) {
        let prev = self.codecs.insert(type_id.into(), Box::new(codec));
        assert!(prev.is_none(), "duplicate custom-value codec registration");
    }

    /// Dormant until the disk cache returns — registration populates the map,
    /// nothing looks up yet.
    #[allow(dead_code)]
    fn codec(&self, type_id: &TypeId) -> Option<&dyn CustomValueCodec> {
        self.codecs.get(type_id).map(|codec| &**codec)
    }
}

impl std::fmt::Debug for CustomValueRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Codecs aren't `Debug`; the registered type ids are the useful state.
        f.debug_struct("CustomValueRegistry")
            .field("types", &self.codecs.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::TypeDef;
    use std::any::Any;
    use std::fmt;

    const BLOB_TYPE: &str = "6c20414f-12a2-4150-bd79-1b4ee23a9f33";

    #[derive(Debug)]
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

    #[test]
    #[should_panic(expected = "duplicate custom-value codec")]
    fn duplicate_registration_panics() {
        let mut registry = CustomValueRegistry::default();
        registry.register(BLOB_TYPE, BlobCodec);
        registry.register(BLOB_TYPE, BlobCodec);
    }
}
