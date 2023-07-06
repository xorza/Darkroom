macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize, Deserialize)]
        pub struct $name(Uuid);

        impl $name {
            pub fn unique() -> $name {
                $name(Uuid::new_v4())
            }
            pub fn nil() -> $name {
                $name(Uuid::nil())
            }
            pub fn is_nil(&self) -> bool {
                self.0 == Uuid::nil()
            }
        }
        impl std::str::FromStr for $name {
            type Err = anyhow::Error;

            fn from_str(id: &str) -> Result<$name, Self::Err> {
                let uuid = Uuid::parse_str(id)?;
                Ok($name(uuid))
            }
        }
        impl Default for $name {
            fn default() -> $name {
                $name::nil()
            }
        }
    };
}
