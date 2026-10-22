//! [`Id`]: a UUID that names one kind of thing.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

/// A UUID that names a `T`: ids of two kinds never mix. In memory an id is its UUID alone, and on
/// the wire a newtype struct around it.
#[repr(transparent)]
pub struct Id<T> {
    uuid: Uuid,
    names: PhantomData<fn() -> T>,
}

/// What an id is in serialized form: a newtype struct around its UUID.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename = "Id")]
struct IdRepr(Uuid);

impl<T> Id<T> {
    /// A random id.
    pub fn unique() -> Self {
        Self::of(Uuid::new_v4())
    }

    pub const fn nil() -> Self {
        Self::of(Uuid::nil())
    }

    pub const fn from_u128(value: u128) -> Self {
        Self::of(Uuid::from_u128(value))
    }

    /// The id a UUID literal spells, for a `const` item: there a malformed literal fails the build
    /// rather than a run.
    ///
    /// # Panics
    /// When `literal` is not a UUID.
    pub const fn literal(literal: &str) -> Self {
        match Uuid::try_parse(literal) {
            Ok(uuid) => Self::of(uuid),
            Err(_) => panic!("invalid UUID literal"),
        }
    }

    pub const fn is_nil(self) -> bool {
        self.uuid.is_nil()
    }

    pub const fn as_u128(self) -> u128 {
        self.uuid.as_u128()
    }

    /// The id as 32 lowercase hex digits with no separator, written into `buf`: a file name.
    pub fn hex(self, buf: &mut [u8; 32]) -> &str {
        self.uuid.simple().encode_lower(buf)
    }

    const fn of(uuid: Uuid) -> Self {
        Self {
            uuid,
            names: PhantomData,
        }
    }
}

impl<T> Clone for Id<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Id<T> {}

impl<T> PartialEq for Id<T> {
    fn eq(&self, other: &Self) -> bool {
        self.uuid == other.uuid
    }
}

impl<T> Eq for Id<T> {}

impl<T> PartialOrd for Id<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Id<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.uuid.cmp(&other.uuid)
    }
}

impl<T> Hash for Id<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.uuid.hash(state);
    }
}

impl<T> Default for Id<T> {
    fn default() -> Self {
        Self::nil()
    }
}

impl<T> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Id").field(&self.uuid).finish()
    }
}

impl<T> fmt::Display for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.uuid, f)
    }
}

impl<T> Serialize for Id<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        IdRepr(self.uuid).serialize(serializer)
    }
}

impl<'de, T> Deserialize<'de> for Id<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        IdRepr::deserialize(deserializer).map(|IdRepr(uuid)| Self::of(uuid))
    }
}

#[cfg(test)]
mod tests {
    use crate::id::Id;
    use crate::serde::serde_format::SerdeFormat;

    #[derive(Debug)]
    struct Sample;

    type SampleId = Id<Sample>;

    const SPELLED: &str = "3effbd19-d4a8-4a9b-a931-78fd0e4f8adb";

    /// An id is the UUID it wraps, in every form it converts through: its 128 bits, its text with
    /// and without hyphens, and each serialized format, where RON writes the newtype struct
    /// around the UUID's string. Ids order as their 128 bits, most significant first.
    #[test]
    fn an_id_is_the_uuid_it_wraps() {
        let id = SampleId::literal(SPELLED);
        assert_eq!(id.to_string(), SPELLED);
        assert_eq!(format!("{id:?}"), format!("Id({SPELLED})"));
        assert_eq!(id.as_u128(), 0x3effbd19_d4a8_4a9b_a931_78fd0e4f8adb);
        assert_eq!(SampleId::from_u128(id.as_u128()), id);
        assert_eq!(id.hex(&mut [0; 32]), "3effbd19d4a84a9ba93178fd0e4f8adb");
        assert!(!id.is_nil());
        assert!(SampleId::default().is_nil());
        assert_eq!(SampleId::default(), SampleId::nil());
        assert!(!SampleId::unique().is_nil());
        assert!(SampleId::from_u128(1) < SampleId::from_u128(1 << 127));
        assert!(SampleId::nil() < SampleId::from_u128(1));
        assert_eq!(
            SerdeFormat::Ron.serialize(&id).unwrap(),
            format!("(\"{SPELLED}\")").into_bytes()
        );
        for format in SerdeFormat::ALL {
            let bytes = format.serialize(&id).unwrap();
            assert_eq!(
                format.deserialize::<SampleId>(&bytes).unwrap(),
                id,
                "{format:?}"
            );
        }
    }
}
