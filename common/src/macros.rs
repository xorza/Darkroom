/// Defines a `#[repr(transparent)]` strongly-typed UUID newtype.
///
/// The expansion references `uuid` and `serde` (with the `derive` feature) in
/// the invoking crate, so **the invoking crate must carry both as
/// dependencies**. They are not `$crate`-qualified: `common` itself does not
/// depend on `uuid`.
///
/// `FromStr` is the fallible parse, for input that may be malformed. A literal id is
/// a `const` item built with `literal`, so a malformed one fails the build.
#[macro_export]
macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Clone,
            Copy,
            PartialEq,
            Eq,
            Ord,
            PartialOrd,
            Debug,
            Hash,
            ::serde::Serialize,
            ::serde::Deserialize,
        )]
        #[repr(transparent)]
        pub struct $name(uuid::Uuid);

        impl $name {
            pub fn unique() -> $name {
                $name(uuid::Uuid::new_v4())
            }
            pub const fn nil() -> $name {
                $name(uuid::Uuid::nil())
            }
            pub const fn from_u128(value: u128) -> $name {
                $name(uuid::Uuid::from_u128(value))
            }
            /// The id a UUID literal spells, for a `const` item: there a malformed
            /// literal fails the build rather than a run.
            ///
            /// # Panics
            /// When `literal` is not a UUID in a form `uuid::Uuid::try_parse` reads.
            pub const fn literal(literal: &str) -> $name {
                match uuid::Uuid::try_parse(literal) {
                    Ok(uuid) => $name(uuid),
                    Err(_) => panic!(concat!("invalid UUID literal for ", stringify!($name))),
                }
            }
            pub const fn is_nil(&self) -> bool {
                self.0.is_nil()
            }
            pub const fn as_u128(&self) -> u128 {
                self.0.as_u128()
            }
            pub const fn as_uuid(&self) -> uuid::Uuid {
                self.0
            }
        }

        impl From<uuid::Uuid> for $name {
            fn from(uuid: uuid::Uuid) -> $name {
                $name(uuid)
            }
        }

        impl From<u128> for $name {
            fn from(value: u128) -> $name {
                $name(uuid::Uuid::from_u128(value))
            }
        }

        impl From<$name> for uuid::Uuid {
            fn from(id: $name) -> uuid::Uuid {
                id.0
            }
        }

        impl AsRef<uuid::Uuid> for $name {
            fn as_ref(&self) -> &uuid::Uuid {
                &self.0
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;

            fn from_str(id: &str) -> std::result::Result<$name, Self::Err> {
                let uuid = uuid::Uuid::parse_str(id)?;
                Ok($name(uuid))
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        impl Default for $name {
            fn default() -> $name {
                $name::nil()
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use crate::serde::serde_format::SerdeFormat;
    use crate::serde::{deserialize, serialize};

    crate::id_type!(SampleId);

    const SPELLED: &str = "3effbd19-d4a8-4a9b-a931-78fd0e4f8adb";

    /// The newtype is the UUID it wraps, in every form it converts through.
    #[test]
    fn an_id_is_the_uuid_it_wraps() {
        let id = SampleId::literal(SPELLED);
        assert_eq!(id.to_string(), SPELLED);
        assert_eq!(SPELLED.parse::<SampleId>().unwrap(), id);
        assert_eq!(SampleId::from_u128(id.as_u128()), id);
        assert_eq!(uuid::Uuid::from(id), id.as_uuid());
        assert!(!id.is_nil());
        assert!(SampleId::default().is_nil());
        assert_eq!(SampleId::default(), SampleId::nil());
        assert!(!SampleId::unique().is_nil());
        for format in SerdeFormat::ALL {
            let bytes = serialize(&id, format).unwrap();
            assert_eq!(
                deserialize::<SampleId>(&bytes, format).unwrap(),
                id,
                "{format:?}"
            );
        }
    }

    /// A malformed id fails with the UUID crate's own error.
    #[test]
    fn a_malformed_id_keeps_the_uuid_error() {
        let input = "not-a-uuid";
        let error: uuid::Error = input.parse::<SampleId>().unwrap_err();
        assert_eq!(
            error.to_string(),
            uuid::Uuid::parse_str(input).unwrap_err().to_string()
        );
    }
}
