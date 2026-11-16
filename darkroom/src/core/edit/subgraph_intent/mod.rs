//! [`SubgraphIntent`]: an edit of the document's set of graphs, rather than of one graph.
//!
//! # Decisions
//!
//! - **A subgraph edit is a document edit**: one undo history, and the change is in every
//!   instance at once, with no publish step inside a document. As Houdini, where "other instances
//!   of the same asset will get the same changes" ([editing
//!   assets](https://www.sidefx.com/docs/houdini/assets/edit.html)). Refused: a draft with Apply, a
//!   mode to enter and leave.
//! - **An edit of the set of graphs is a request of its own**, committed into the same history
//!   entry as the graph edits of its batch, so a collapse, its new definition and the instance that
//!   places it are one undo. Refused: a graph intent on the main graph that writes other graphs,
//!   which hides what it touches.

use std::mem;

use scenarium::{Interface, Library, Manifest, RefusedFate, Subgraph, SubgraphDigest, SubgraphId};

use crate::core::document::Document;
use crate::core::document::subgraph_side::SubgraphSide;
use crate::core::edit::bring_in::{self, Carried, bring_in};
use crate::core::edit::error::{EditRefusal, IntentError};
use crate::core::edit::step::change::Change;
use crate::core::edit::step::set_definition::SetDefinition;
use crate::core::edit::step::set_interface::SetInterface;
use crate::core::edit::step::set_side::SetSide;
use crate::core::edit::step::set_subgraph::SetSubgraph;
use crate::core::edit::step::set_subgraph_name::SetSubgraphName;
use crate::core::edit::step::undo_step::UndoStep;

/// An edit of a subgraph's definition: one coming in or going out, its interface or its name.
/// Travels as a
/// [`DocumentRequest::Subgraph`](crate::core::edit::document_request::DocumentRequest::Subgraph)
/// and lands in the same history entry as the graph edits of its batch, as a collapse's new
/// definition lands with the instance that places it.
#[derive(Debug)]
pub(crate) enum SubgraphIntent {
    /// Put a new definition in, with its side. Boxed: a whole definition dwarfs the rest.
    Add(Box<NewSubgraph>),
    /// Take out a definition no node places.
    Remove { id: SubgraphId },
    /// Set a subgraph's interface: what each instance and both boundary nodes show, dropping
    /// each binding the new one does not hold, in every graph.
    SetInterface { id: SubgraphId, to: Interface },
    /// Rename a subgraph to `to`, trimmed: a blank name keeps the one it has, and one another
    /// subgraph holds takes the next free number, as Make Unique names its copy.
    Rename { id: SubgraphId, to: String },
    /// Mark a subgraph shared, so the library lists it while the document is a library file, or
    /// take the mark off.
    SetShared { id: SubgraphId, shared: bool },
    /// Put the library's definition in place of the copy held under its id, keeping each
    /// instance's bindings that the library's interface holds. Boxed: whole definitions.
    Update(Box<Update>),
    /// Record `digest` as the one subgraph `id` last shared with the library: what a publish
    /// leaves.
    Synced {
        id: SubgraphId,
        digest: SubgraphDigest,
    },
}

/// What [`SubgraphIntent::Update`] puts in: the library's definition under the copy's id, last,
/// after each definition it places, and the manifest of the file they came from.
#[derive(Debug)]
pub(crate) struct Update {
    pub(crate) id: SubgraphId,
    pub(crate) carried: Vec<Carried>,
    pub(crate) manifest: Manifest,
}

/// A definition [`SubgraphIntent::Add`] puts in.
#[derive(Clone, Debug)]
pub(crate) struct NewSubgraph {
    pub(crate) id: SubgraphId,
    pub(crate) subgraph: Subgraph,
    pub(crate) side: SubgraphSide,
}

impl SubgraphIntent {
    /// Build and apply this edit against `document`, pushing its step onto `steps`: none for a
    /// stale one, as a subgraph gone since the request was raised, or a no-op. A removal of a
    /// subgraph nodes still place is refused.
    ///
    /// # Panics
    /// On an `Add` under an id the document holds: ids are minted fresh.
    pub(crate) fn commit(
        self,
        document: &mut Document,
        library: &Library,
        steps: &mut Vec<UndoStep>,
    ) -> Result<(), IntentError> {
        let mut land = |step: UndoStep, document: &mut Document| {
            if !step.is_noop() {
                step.apply(document);
                steps.push(step);
            }
        };
        let step = match self {
            Self::Add(new) => {
                let NewSubgraph { id, subgraph, side } = *new;
                assert!(
                    document.graphs.subgraph(id).is_none(),
                    "a new subgraph takes a fresh id"
                );
                UndoStep::Subgraph(Box::new(SetSubgraph::insertion(id, subgraph, side)))
            }
            Self::Remove { id } => {
                if document.graphs.subgraph(id).is_none() {
                    return Ok(());
                }
                let uses = document.graphs.uses(id).count();
                if uses > 0 {
                    return Err(EditRefusal::SubgraphInUse { uses }.into());
                }
                UndoStep::Subgraph(Box::new(SetSubgraph::removal(document, id)))
            }
            Self::SetInterface { id, to } => {
                if document.graphs.subgraph(id).is_none() {
                    return Ok(());
                }
                UndoStep::Interface(Box::new(SetInterface::plan(
                    document,
                    id,
                    to,
                    library,
                    RefusedFate::Drop,
                )))
            }
            Self::Rename { id, to } => {
                let Some(subgraph) = document.graphs.subgraph(id) else {
                    return Ok(());
                };
                let wanted = to.trim();
                if wanted.is_empty() || wanted == subgraph.name {
                    return Ok(());
                }
                UndoStep::SubgraphName(SetSubgraphName {
                    id,
                    name: Change {
                        from: subgraph.name.clone(),
                        to: document.free_subgraph_name(wanted),
                    },
                })
            }
            Self::SetShared { id, shared } => {
                let Some(side) = document.sides.get(&id) else {
                    return Ok(());
                };
                UndoStep::Side(SetSide {
                    id,
                    shared: Change {
                        from: side.shared,
                        to: shared,
                    },
                    synced: Change {
                        from: side.synced,
                        to: side.synced,
                    },
                })
            }
            Self::Synced { id, digest } => {
                let Some(side) = document.sides.get(&id) else {
                    return Ok(());
                };
                UndoStep::Side(SetSide {
                    id,
                    shared: Change {
                        from: side.shared,
                        to: side.shared,
                    },
                    synced: Change {
                        from: side.synced,
                        to: Some(digest),
                    },
                })
            }
            Self::Update(update) => {
                let Update {
                    id,
                    mut carried,
                    manifest,
                } = *update;
                if document.graphs.subgraph(id).is_none() {
                    return Ok(());
                }
                // The definition itself is parsed against this library with those it places, as
                // each of them is: a node it holds of a func this build lacks is a ghost node.
                bring_in::reconcile(&mut carried, library, &manifest);
                let definition = carried
                    .pop()
                    .expect("an update carries its definition last");
                assert_eq!(
                    definition.id, id,
                    "an update puts in the definition under its id"
                );
                let mut brought = bring_in(document, library, carried, &manifest);
                for NewSubgraph { id, subgraph, side } in mem::take(&mut brought.subgraphs) {
                    land(
                        UndoStep::Subgraph(Box::new(SetSubgraph::insertion(id, subgraph, side))),
                        document,
                    );
                }
                let mut subgraph = definition.subgraph;
                brought.enter(&mut subgraph, library);
                let interface = subgraph.interface.clone();
                land(
                    UndoStep::Interface(Box::new(SetInterface::plan(
                        document,
                        id,
                        interface,
                        library,
                        RefusedFate::Ghost,
                    ))),
                    document,
                );
                let side = &document.sides[&id];
                let synced = side.synced;
                land(
                    UndoStep::Definition(Box::new(SetDefinition {
                        id,
                        definition: Change {
                            from: document.graphs.subgraph(id).expect("checked above").clone(),
                            to: subgraph,
                        },
                        view: Change {
                            from: side.view.clone(),
                            to: definition.view,
                        },
                    })),
                    document,
                );
                UndoStep::Side(SetSide {
                    id,
                    shared: Change {
                        from: document.sides[&id].shared,
                        to: document.sides[&id].shared,
                    },
                    synced: Change {
                        from: synced,
                        to: definition.synced,
                    },
                })
            }
        };
        land(step, document);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
