//! [`CustomType`]: a registered custom value type, its name and its disk codec.

use std::sync::Arc;

use common::Id;

use crate::data::codec::CustomValueCodec;

/// The id of a custom value type a [`DataType::Custom`](crate::DataType::Custom) names, and that
/// a [`CustomValue`](crate::CustomValue) reports.
pub type CustomTypeId = Id<CustomType>;

/// A custom value type a library registers: what the editor shows, and the codec that writes
/// its values to the disk cache, if any. A type with no codec is not cached on disk.
#[derive(Clone, Debug)]
pub struct CustomType {
    name: String,
    codec: Option<Arc<dyn CustomValueCodec>>,
}

impl CustomType {
    /// A custom type named `name` with no disk codec.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            codec: None,
        }
    }

    /// A custom type named `name` whose values `codec` writes to the disk cache.
    pub fn with_codec(name: impl Into<String>, codec: Arc<dyn CustomValueCodec>) -> Self {
        Self {
            name: name.into(),
            codec: Some(codec),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub(crate) const fn codec(&self) -> Option<&Arc<dyn CustomValueCodec>> {
        self.codec.as_ref()
    }

    /// Whether `other` declares this same type: the same name and the same codec instance. No
    /// path registers one type from two instances, as `lens` registers `Image` once. Refused:
    /// identity by the codec's `version()`, which two different codecs share.
    pub(crate) fn same_as(&self, other: &Self) -> bool {
        self.name == other.name
            && match (&self.codec, &other.codec) {
                (None, None) => true,
                (Some(codec), Some(other)) => Arc::ptr_eq(codec, other),
                _ => false,
            }
    }
}
