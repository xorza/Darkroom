use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash, Serialize, Deserialize)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> $name {
                $name(Uuid::new_v4())
            }
            pub fn nil() -> $name {
                $name(Uuid::nil())
            }
            pub fn is_nil(&self) -> bool {
                self.0 == Uuid::nil()
            }
            pub fn from_str(id: &str) -> anyhow::Result<$name> {
                let uuid = Uuid::parse_str(id)?;
                Ok($name(uuid))
            }
        }
    };
}
