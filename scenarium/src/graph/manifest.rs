//! [`Manifest`]: the saved declaration of each func a file's graphs name.
//!
//! # Decisions
//!
//! - **The file carries a manifest, and memory holds none.** A save derives it from the library
//!   ([`GraphSet::manifest`](crate::GraphSet::manifest)) and a load spends it
//!   ([`GraphSet::reconcile`](crate::GraphSet::reconcile)): it is how a load whose library lost
//!   a func, or a port of one, still knows what the func showed. As `Cargo.lock` records what a
//!   build resolved, and as ComfyUI writes each node's inputs and outputs with name and type
//!   into the workflow ([workflow JSON](https://docs.comfy.org/specs/workflow_json)). Refused:
//!   the declaration on every node, as ComfyUI, Blender and Unreal write it, one copy for each
//!   node of a func; a table kept in memory for live funcs, a copy of the library that each edit
//!   and each undo has to keep in step.
//! - **What a load learns from it, it moves onto what needs it**: a ghost node's saved
//!   declaration, a ghost's saved port. So a definition with its ghosts is one value in a clip,
//!   an undo step and a library file. Refused: missing funcs' declarations in a table of the
//!   set, which a compound travels without. The names of types are the exception
//!   ([`TypeNames`]): an id names one type for good, so the set keeps them aside.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::graph::identity::FuncId;
use crate::graph::saved_decl::SavedDecl;
use crate::graph::type_names::TypeNames;

/// The saved declaration of each func a file's graphs name, and the name of each type its ports
/// name, as the library gave them at the save.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(deserialize_with = "crate::graph::serde::deserialize_manifest_funcs")]
    funcs: BTreeMap<FuncId, SavedDecl>,
    types: TypeNames,
}

impl Manifest {
    /// The name of each registered type the file's ports name.
    pub const fn types(&self) -> &TypeNames {
        &self.types
    }

    /// This manifest naming its types as `types` does.
    pub(crate) fn with_types(self, types: TypeNames) -> Self {
        Self { types, ..self }
    }

    /// The saved declaration of func `id`, if the file named a func under it.
    pub fn func(&self, id: FuncId) -> Option<&SavedDecl> {
        self.funcs.get(&id)
    }

    pub fn len(&self) -> usize {
        self.funcs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.funcs.is_empty()
    }

    /// In func-id order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (FuncId, &SavedDecl)> {
        self.funcs.iter().map(|(id, saved)| (*id, saved))
    }
}

impl FromIterator<(FuncId, SavedDecl)> for Manifest {
    /// The manifest of `funcs`, the first declaration given for an id kept.
    fn from_iter<I: IntoIterator<Item = (FuncId, SavedDecl)>>(funcs: I) -> Self {
        let mut held = BTreeMap::new();
        for (id, saved) in funcs {
            held.entry(id).or_insert(saved);
        }
        Self {
            funcs: held,
            types: TypeNames::default(),
        }
    }
}
