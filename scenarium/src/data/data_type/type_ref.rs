//! [`TypeRef`]: a type the library registers under an id.

use serde::{Deserialize, Serialize};

use crate::data::data_type::DataType;
use crate::data::data_type::custom_type::CustomTypeId;
use crate::data::data_type::enum_type::EnumId;
use crate::data::data_type::file_kind::FileKindId;

/// A type the library registers under an id, as opposed to a scalar every build knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TypeRef {
    Custom(CustomTypeId),
    Enum(EnumId),
    /// A file kind, which names a file type in either mode.
    FileKind(FileKindId),
}

impl TypeRef {
    /// The registered type `data_type` names, `None` for a scalar.
    pub const fn of(data_type: DataType) -> Option<Self> {
        match data_type {
            DataType::Any
            | DataType::Float
            | DataType::Int
            | DataType::Bool
            | DataType::String
            | DataType::Dir => None,
            DataType::File { kind, .. } => Some(Self::FileKind(kind)),
            DataType::Custom(id) => Some(Self::Custom(id)),
            DataType::Enum(id) => Some(Self::Enum(id)),
        }
    }
}
