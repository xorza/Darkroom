//! [`CompoundIntent`]: an edit of the document's set of graphs, rather than of one graph.
//!
//! # Decisions
//!
//! - **A compound edit is a document edit**: one undo history, and the change is in every
//!   instance at once, with no publish step inside a document. As Houdini, where "other instances
//!   of the same asset will get the same changes" ([editing
//!   assets](https://www.sidefx.com/docs/houdini/assets/edit.html)). Refused: a draft with Apply, a
//!   mode to enter and leave.
//! - **An edit of the set of graphs is a request of its own**, committed into the same history
//!   entry as the graph edits of its batch, so a collapse, its new definition and the instance that
//!   places it are one undo. Refused: a graph intent on the main graph that writes other graphs,
//!   which hides what it touches.

use std::mem;

use scenarium::{
    Compound, CompoundDigest, CompoundId, GraphId, Interface, Library, Manifest, RefusedFate,
    TypeNames,
};

use crate::core::document::Document;
use crate::core::document::compound_side::{CompoundSide, SideMarks};
use crate::core::edit::bring_in::{self, Carried, bring_in};
use crate::core::edit::error::{EditRefusal, IntentError};
use crate::core::edit::step::change::Change;
use crate::core::edit::step::set_compound::SetCompound;
use crate::core::edit::step::set_compound_name::SetCompoundName;
use crate::core::edit::step::set_definition::SetDefinition;
use crate::core::edit::step::set_interface::SetInterface;
use crate::core::edit::step::set_side::SetSide;
use crate::core::edit::step::undo_step::UndoStep;

/// An edit of a compound's definition: one coming in or going out, its interface or its name.
/// Travels as a
/// [`DocumentRequest::Compound`](crate::core::edit::document_request::DocumentRequest::Compound)
/// and lands in the same history entry as the graph edits of its batch, as a collapse's new
/// definition lands with the instance that places it.
#[derive(Debug)]
pub(crate) enum CompoundIntent {
    /// Put a new definition in, with its side. Boxed: a whole definition dwarfs the rest.
    Add(Box<NewCompound>),
    /// Take out a definition no node places.
    Remove { id: CompoundId },
    /// Set a compound's interface: what each instance and both boundary nodes show, dropping
    /// each binding the new one does not hold, in every graph. Boxed: an interface dwarfs the
    /// rest.
    SetInterface { id: CompoundId, to: Box<Interface> },
    /// Rename a compound to `to`, trimmed: a blank name keeps the one it has, and one another
    /// compound holds takes the next free number, as Make Unique names its copy.
    Rename { id: CompoundId, to: String },
    /// Mark a compound shared, so the library lists it while the document is a library file, or
    /// take the mark off.
    SetShared { id: CompoundId, shared: bool },
    /// Put the library's definition in place of the copy held under its id, keeping each
    /// instance's bindings that the library's interface holds. Boxed: whole definitions.
    Update(Box<Update>),
    /// Record `digest` as the one compound `id` last shared with the library: what a publish
    /// leaves.
    Synced {
        id: CompoundId,
        digest: CompoundDigest,
    },
    /// Make compound `id` follow the library, read-only, or Make Local.
    SetFollows { id: CompoundId, follows: bool },
    /// Lock compound `id` against edits, or Unlock it.
    SetLocked { id: CompoundId, locked: bool },
}

/// What [`CompoundIntent::Update`] puts in: the library's definition under the copy's id, last,
/// after each definition it places, and the manifest of the file they came from.
#[derive(Debug)]
pub(crate) struct Update {
    pub(crate) id: CompoundId,
    pub(crate) carried: Vec<Carried>,
    pub(crate) manifest: Manifest,
}

/// A definition [`CompoundIntent::Add`] puts in.
#[derive(Clone, Debug)]
pub(crate) struct NewCompound {
    pub(crate) id: CompoundId,
    pub(crate) compound: Compound,
    pub(crate) side: CompoundSide,
    /// The names the file it came from gave its types, which the set learns
    /// ([`GraphSet::learn_type_names`](scenarium::GraphSet::learn_type_names)); none for one
    /// made here.
    pub(crate) type_names: TypeNames,
}

impl CompoundIntent {
    /// Build and apply this edit against `document`, pushing its step onto `steps`: none for a
    /// stale one, as a compound gone since the request was raised, or a no-op. A removal of a
    /// compound nodes still place is refused.
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
        if let Self::SetInterface { id, .. } | Self::Rename { id, .. } = &self
            && let Some(why) = document.read_only(GraphId::Compound(*id))
        {
            return Err(EditRefusal::ReadOnly { why }.into());
        }
        let step = match self {
            Self::Add(new) => {
                let NewCompound {
                    id,
                    compound,
                    side,
                    type_names,
                } = *new;
                assert!(
                    document.graphs.compound(id).is_none(),
                    "a new compound takes a fresh id"
                );
                // A cache of what files said, outside the undo (`TypeNames`).
                document.graphs.learn_type_names(&type_names, library);
                UndoStep::Compound(Box::new(SetCompound::insertion(id, compound, side)))
            }
            Self::Remove { id } => {
                if document.graphs.compound(id).is_none() {
                    return Ok(());
                }
                let uses = document.graphs.uses(id).count();
                if uses > 0 {
                    return Err(EditRefusal::CompoundInUse { uses }.into());
                }
                UndoStep::Compound(Box::new(SetCompound::removal(document, id)))
            }
            Self::SetInterface { id, to } => {
                if document.graphs.compound(id).is_none() {
                    return Ok(());
                }
                UndoStep::Interface(Box::new(SetInterface::plan(
                    document,
                    id,
                    *to,
                    library,
                    RefusedFate::Drop,
                )))
            }
            Self::Rename { id, to } => {
                let Some(compound) = document.graphs.compound(id) else {
                    return Ok(());
                };
                let wanted = to.trim();
                if wanted.is_empty() || wanted == compound.decl.name {
                    return Ok(());
                }
                UndoStep::CompoundName(SetCompoundName {
                    id,
                    name: Change {
                        from: compound.decl.name.clone(),
                        to: document.free_compound_name(wanted),
                    },
                })
            }
            Self::SetShared { id, shared } => {
                let Some(step) = mark(document, id, |marks| marks.shared = shared) else {
                    return Ok(());
                };
                step
            }
            Self::Synced { id, digest } => {
                let Some(step) = mark(document, id, |marks| marks.synced = Some(digest)) else {
                    return Ok(());
                };
                step
            }
            Self::SetFollows { id, follows } => {
                let Some(step) = mark(document, id, |marks| marks.follows = follows) else {
                    return Ok(());
                };
                step
            }
            Self::SetLocked { id, locked } => {
                let Some(step) = mark(document, id, |marks| marks.locked = locked) else {
                    return Ok(());
                };
                step
            }
            Self::Update(update) => {
                let Update {
                    id,
                    mut carried,
                    manifest,
                } = *update;
                if document.graphs.compound(id).is_none() {
                    return Ok(());
                }
                // The definition itself is parsed against this library with those it places, as
                // each of them is: a node it holds of a func this build lacks is a ghost node.
                bring_in::reconcile(&mut carried, library, &manifest);
                document.graphs.learn_type_names(manifest.types(), library);
                let definition = carried
                    .pop()
                    .expect("an update carries its definition last");
                assert_eq!(
                    definition.id, id,
                    "an update puts in the definition under its id"
                );
                let mut brought = bring_in(document, library, carried, &manifest);
                for NewCompound {
                    id, compound, side, ..
                } in mem::take(&mut brought.compounds)
                {
                    land(
                        UndoStep::Compound(Box::new(SetCompound::insertion(id, compound, side))),
                        document,
                    );
                }
                let mut compound = definition.compound;
                brought.enter(&mut compound, library);
                let interface = compound.decl.interface.clone();
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
                            from: document.graphs.compound(id).expect("checked above").into(),
                            to: compound,
                        },
                        view: Change {
                            from: side.view.clone(),
                            to: definition.view,
                        },
                    })),
                    document,
                );
                let from = document.sides[&id].marks();
                UndoStep::Side(SetSide {
                    id,
                    marks: Change {
                        from: SideMarks { synced, ..from },
                        to: SideMarks {
                            synced: definition.synced,
                            ..from
                        },
                    },
                })
            }
        };
        land(step, document);
        Ok(())
    }
}

/// The step that changes compound `id`'s marks as `edit` does, or `None` for a compound the
/// document no longer holds.
fn mark(
    document: &Document,
    id: CompoundId,
    edit: impl FnOnce(&mut SideMarks),
) -> Option<UndoStep> {
    let from = document.sides.get(&id)?.marks();
    let mut to = from;
    edit(&mut to);
    Some(UndoStep::Side(SetSide {
        id,
        marks: Change { from, to },
    }))
}

#[cfg(test)]
mod tests;
