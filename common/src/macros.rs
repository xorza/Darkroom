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
