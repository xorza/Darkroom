#[macro_export]
macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, Ord, PartialOrd, Debug, Hash, serde::Serialize, serde::Deserialize)]
        pub struct $name(uuid::Uuid);

        impl $name {
            pub fn unique() -> $name {
                $name(uuid::Uuid::new_v4())
            }
            pub fn nil() -> $name {
                $name(uuid::Uuid::nil())
            }
            pub fn is_nil(&self) -> bool {
                self.0 == uuid::Uuid::nil()
            }
        }
        impl std::str::FromStr for $name {
            type Err = anyhow::Error;

            fn from_str(id: &str) -> Result<$name, Self::Err> {
                let uuid = uuid::Uuid::parse_str(id)?;
                Ok($name(uuid))
            }
        }
        // impl std::string::ToString for $name {
        //     fn to_string(&self) -> String {
        //         self.0.to_string()
        //     }
        // }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0.to_string())
            }
        }
        impl Default for $name {
            fn default() -> $name {
                $name::nil()
            }
        }
    };
}
