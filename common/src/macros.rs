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
            serde::Serialize,
            serde::Deserialize,
        )]
        #[repr(transparent)]
        pub struct $name(uuid::Uuid);

        impl $name {
            /// Namespace UUID for generating deterministic IDs from names.
            /// This is a custom namespace specific to this application.
            const NAMESPACE: uuid::Uuid =
                uuid::Uuid::from_u128(0x3063f616_7396_43bc_afcb_b57bb6913305);

            pub fn unique() -> $name {
                $name(uuid::Uuid::new_v4())
            }
            pub fn nil() -> $name {
                $name(uuid::Uuid::nil())
            }
            /// Creates a deterministic UUID v5 from a type.
            /// The same type will always produce the same UUID.
            pub fn from_type<T: ?Sized>() -> $name {
                $name(uuid::Uuid::new_v5(
                    &Self::NAMESPACE,
                    std::any::type_name::<T>().as_bytes(),
                ))
            }
            pub const fn from_u128(value: u128) -> $name {
                $name(uuid::Uuid::from_u128(value))
            }
            pub fn is_nil(&self) -> bool {
                self.0 == uuid::Uuid::nil()
            }
            pub fn as_u128(&self) -> u128 {
                self.0.as_u128()
            }
            pub fn as_u64_pair(&self) -> (u64, u64) {
                self.0.as_u64_pair()
            }
            pub fn as_uuid(&self) -> uuid::Uuid {
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
            type Err = anyhow::Error;

            fn from_str(id: &str) -> std::result::Result<$name, Self::Err> {
                let uuid = uuid::Uuid::parse_str(id)?;
                Ok($name(uuid))
            }
        }

        impl From<&str> for $name {
            fn from(id: &str) -> $name {
                let uuid = uuid::Uuid::parse_str(id)
                    .expect(concat!("invalid UUID string for ", stringify!($name)));
                $name(uuid)
            }
        }

        impl From<String> for $name {
            fn from(id: String) -> $name {
                id.as_str().into()
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
