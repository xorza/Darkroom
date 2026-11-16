//! [`TypeNames`]: the names of registered types, by id, as a file saved them.
//!
//! # Decisions
//!
//! - **A file names each type its ports name**, beside the funcs in its
//!   [`Manifest`](crate::Manifest), as the library gave it at the save. So a ghost port and an
//!   interface port of a type a later build lacks still read its name. A type stays what the
//!   library says it is, "equal only to itself": the name is what it shows, with no variants and
//!   no codec.
//! - **What a load learns of a type the library lacks, the set keeps until the end of the
//!   session** ([`GraphSet::type_name`](crate::GraphSet::type_name)), and the next save writes
//!   the names its ports still need. A type's id never changes its meaning, so a name kept for
//!   an id nothing names any more is unseen, and an undo leaves the table as it is: a cache of
//!   what files said, filled from them alone. Refused: the name on each port, a copy for each
//!   port of a library entry; the table as part of the document, which every edit and undo would
//!   have to keep in step.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::data::data_type::DataType;
use crate::data::data_type::custom_type::CustomTypeId;
use crate::data::data_type::enum_type::EnumId;
use crate::data::data_type::file_kind::FileKindId;
use crate::library::Library;

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

/// Names of registered types, by id: a file's, and those a session learned from files.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TypeNames {
    #[serde(deserialize_with = "crate::graph::serde::deserialize_type_names")]
    names: BTreeMap<TypeRef, String>,
}

impl TypeNames {
    pub fn get(&self, type_ref: TypeRef) -> Option<&str> {
        self.names.get(&type_ref).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// In id order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (TypeRef, &str)> {
        self.names
            .iter()
            .map(|(type_ref, name)| (*type_ref, name.as_str()))
    }

    /// Learn each name of `names` for a type `library` does not register; one already known
    /// stays, as an id names one type.
    pub(crate) fn learn(&mut self, names: &Self, library: &Library) {
        for (type_ref, name) in names.iter() {
            if !library.knows_type(type_ref) {
                self.names
                    .entry(type_ref)
                    .or_insert_with(|| name.to_owned());
            }
        }
    }
}

impl<'a> FromIterator<(TypeRef, &'a str)> for TypeNames {
    /// The first name given for an id kept.
    fn from_iter<I: IntoIterator<Item = (TypeRef, &'a str)>>(names: I) -> Self {
        let mut held = BTreeMap::new();
        for (type_ref, name) in names {
            held.entry(type_ref).or_insert_with(|| name.to_owned());
        }
        Self { names: held }
    }
}
