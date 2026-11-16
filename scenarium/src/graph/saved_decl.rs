//! [`SavedDecl`]: what a document remembers of a declaration, enough to draw it and to tell what
//! changed, nothing to run.
//!
//! # Decisions
//!
//! - **The word is "saved"**, the form a thing has in the file, as darkroom's `SavedPaths`.
//!   Refused: "outline", which is a glyph's ring in the editor and a shape's edge in `palantir`.
//! - **A saved declaration holds a port's id, name, type and default, and no more.** Refused:
//!   the whole [`Ports`](crate::Ports) of a func, which writes every tooltip and picker list
//!   into each document that names it.

use serde::{Deserialize, Serialize};

use crate::data::const_value::ConstValue;
use crate::data::data_type::DataType;
use crate::graph::identity::PortId;
use crate::graph::node_decl::NodeDecl;
use crate::graph::ports::Numbered;

/// A port as its declaration last showed it: its name and its type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedPort {
    pub name: String,
    pub data_type: DataType,
}

/// A port as the graph shows it: its name and its type, borrowed from wherever they are known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShownPort<'a> {
    pub name: &'a str,
    pub data_type: DataType,
}

impl<'a> From<&'a SavedPort> for ShownPort<'a> {
    fn from(saved: &'a SavedPort) -> Self {
        Self {
            name: &saved.name,
            data_type: saved.data_type,
        }
    }
}

/// An input as its declaration last showed it, with the default it gave a new node. Flat, not
/// a [`SavedPort`] with a default beside it, as a file writes one for each input of each func
/// it names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedInput {
    pub name: String,
    pub data_type: DataType,
    pub default_value: Option<ConstValue>,
}

impl SavedInput {
    /// The input's name and type: what a ghost keeps of an input its node lost.
    pub fn port(&self) -> SavedPort {
        SavedPort {
            name: self.name.clone(),
            data_type: self.data_type,
        }
    }
}

/// An event as its declaration last showed it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedEvent {
    pub name: String,
}

/// A declaration as a document last saw it: its name, its category, and its ports and events
/// in the order it showed them. What a [`Manifest`](crate::Manifest) holds of each func a file
/// names, and what a [`GhostNode`](crate::GhostNode) holds of the node it was.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedDecl {
    pub name: String,
    /// Empty for a declaration with none: a subgraph's.
    pub category: String,
    pub inputs: Vec<Numbered<SavedInput>>,
    pub outputs: Vec<Numbered<SavedPort>>,
    pub events: Vec<Numbered<SavedEvent>>,
}

impl SavedDecl {
    /// The input under `id`, if the declaration showed one.
    pub fn input(&self, id: PortId) -> Option<&SavedInput> {
        self.inputs
            .iter()
            .find(|input| input.id == id)
            .map(|input| &input.port)
    }

    /// The output under `id`, if the declaration showed one.
    pub fn output(&self, id: PortId) -> Option<&SavedPort> {
        self.outputs
            .iter()
            .find(|output| output.id == id)
            .map(|output| &output.port)
    }

    /// The event under `id`, if the declaration fired one.
    pub fn event(&self, id: PortId) -> Option<&SavedEvent> {
        self.events
            .iter()
            .find(|event| event.id == id)
            .map(|event| &event.port)
    }
}

impl NodeDecl<'_> {
    /// This declaration as a document remembers it.
    pub fn saved(self) -> SavedDecl {
        SavedDecl {
            name: self.name().to_owned(),
            category: self
                .func()
                .map_or_else(String::new, |func| func.category().to_owned()),
            inputs: self
                .inputs()
                .iter()
                .map(|input| {
                    Numbered::new(
                        input.id,
                        SavedInput {
                            name: input.name.clone(),
                            data_type: input.data_type,
                            default_value: input.default_value.clone(),
                        },
                    )
                })
                .collect(),
            outputs: self
                .outputs()
                .iter()
                .map(|output| {
                    Numbered::new(
                        output.id,
                        SavedPort {
                            name: output.name.clone(),
                            data_type: output.data_type,
                        },
                    )
                })
                .collect(),
            events: self
                .events()
                .iter()
                .map(|event| {
                    Numbered::new(
                        event.id,
                        SavedEvent {
                            name: event.name.clone(),
                        },
                    )
                })
                .collect(),
        }
    }
}
