use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use strum::VariantNames;

use common::id_type;

use crate::StaticValue;

pub trait EnumVariants {
    fn variant_names() -> Vec<String>;
}

impl<T: VariantNames> EnumVariants for T {
    fn variant_names() -> Vec<String> {
        T::VARIANTS
            .iter()
            .map(|variant| (*variant).to_string())
            .collect()
    }
}

id_type!(TypeId);

impl TypeId {
    pub fn from_name(namespace: TypeId, name: &str) -> Self {
        uuid::Uuid::new_v5(&namespace.as_uuid(), name.as_bytes()).into()
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FsPathMode {
    #[default]
    ExistingFile,
    ExistingFiles,
    NewFile,
    Directory,
}

#[derive(Clone, Default, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FsPathConfig {
    pub mode: FsPathMode,
    pub extensions: Vec<String>,
}

impl FsPathConfig {
    pub fn new(mode: FsPathMode) -> Self {
        Self {
            mode,
            extensions: Vec::new(),
        }
    }

    pub fn with_extensions(mode: FsPathMode, extensions: Vec<String>) -> Self {
        Self { mode, extensions }
    }
}

#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataType {
    #[default]
    Any,
    Float,
    Int,
    Bool,
    String,
    FsPath(Arc<FsPathConfig>),
    Custom(TypeId),
    Enum(TypeId),
}

impl DataType {
    pub fn default_value(&self) -> Option<StaticValue> {
        Some(match self {
            DataType::Any => StaticValue::Null,
            DataType::Float => StaticValue::Float(0.0),
            DataType::Int => StaticValue::Int(0),
            DataType::Bool => StaticValue::Bool(false),
            DataType::String => StaticValue::String(String::new()),
            DataType::FsPath(config) => match config.mode {
                FsPathMode::ExistingFiles => StaticValue::FsPaths(Vec::new()),
                FsPathMode::ExistingFile | FsPathMode::NewFile | FsPathMode::Directory => {
                    StaticValue::FsPath(String::new())
                }
            },
            DataType::Custom(_) | DataType::Enum(_) => return None,
        })
    }

    pub fn compatible_with(&self, source: &DataType) -> bool {
        if matches!(self, DataType::Any) || matches!(source, DataType::Any) {
            return true;
        }
        if self.is_numeric_scalar() && source.is_numeric_scalar() {
            return true;
        }
        self == source
    }

    fn is_numeric_scalar(&self) -> bool {
        matches!(self, DataType::Float | DataType::Int | DataType::Bool)
    }
}

impl FromStr for DataType {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "float" | "number" => Ok(DataType::Float),
            "int" => Ok(DataType::Int),
            "bool" => Ok(DataType::Bool),
            "string" => Ok(DataType::String),
            "path" => Ok(DataType::FsPath(Arc::new(FsPathConfig::default()))),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_id_from_name_is_deterministic_namespaced_and_v5() {
        let namespace_a = TypeId::from_u128(0x11);
        let namespace_b = TypeId::from_u128(0x22);

        assert_eq!(
            TypeId::from_name(namespace_a, "Mode"),
            TypeId::from_name(namespace_a, "Mode")
        );
        assert_ne!(
            TypeId::from_name(namespace_a, "Mode"),
            TypeId::from_name(namespace_a, "Other")
        );
        assert_ne!(
            TypeId::from_name(namespace_a, "Mode"),
            TypeId::from_name(namespace_b, "Mode")
        );

        let id = TypeId::from_name(namespace_a, "Mode");
        assert_eq!((id.as_u128() >> 76) & 0xf, 5);
        assert_ne!(id.as_u128() >> 64, 0);
    }

    #[test]
    fn compatibility_and_defaults_follow_runtime_coercions() {
        let custom = |id| DataType::Custom(TypeId::from_u128(id));

        assert!(DataType::Float.compatible_with(&DataType::Int));
        assert!(DataType::Bool.compatible_with(&DataType::Float));
        assert!(DataType::Any.compatible_with(&DataType::String));
        assert!(!DataType::String.compatible_with(&DataType::Bool));
        assert!(custom(1).compatible_with(&custom(1)));
        assert!(!custom(1).compatible_with(&custom(2)));

        assert_eq!(
            DataType::Float.default_value(),
            Some(StaticValue::Float(0.0))
        );
        assert_eq!(DataType::Int.default_value(), Some(StaticValue::Int(0)));
        assert_eq!(
            DataType::Bool.default_value(),
            Some(StaticValue::Bool(false))
        );
        assert_eq!(
            DataType::FsPath(Arc::new(FsPathConfig::default())).default_value(),
            Some(StaticValue::FsPath(String::new()))
        );
        assert_eq!(
            DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::ExistingFiles)))
                .default_value(),
            Some(StaticValue::FsPaths(Vec::new()))
        );
        assert_eq!(custom(1).default_value(), None);
        assert_eq!(DataType::Enum(TypeId::from_u128(2)).default_value(), None);
    }
}
