//! [`SubgraphIntent`]: an edit of the document's set of graphs, rather than of one graph.

use scenarium::{Interface, Library, Subgraph, SubgraphId};

use crate::core::document::Document;
use crate::core::document::subgraph_side::SubgraphSide;
use crate::core::edit::error::{EditRefusal, IntentError};
use crate::core::edit::step::set_interface::SetInterface;
use crate::core::edit::step::set_subgraph::SetSubgraph;
use crate::core::edit::step::undo_step::UndoStep;

/// An edit of a subgraph's definition: one coming in or going out, or its interface. Travels as
/// a [`DocumentRequest::Subgraph`](crate::core::edit::document_request::DocumentRequest::Subgraph)
/// and lands in the same history entry as the graph edits of its batch, as Make Unique's copy
/// lands with the instance it repoints.
#[derive(Debug)]
pub(crate) enum SubgraphIntent {
    /// Put a new definition in, with its side. Boxed: a whole definition dwarfs the rest.
    Add(Box<NewSubgraph>),
    /// Take out a definition no node places.
    Remove { id: SubgraphId },
    /// Set a subgraph's interface: what each instance and both boundary nodes show, dropping
    /// each binding the new one does not hold, in every graph.
    SetInterface { id: SubgraphId, to: Interface },
}

/// A definition [`SubgraphIntent::Add`] puts in.
#[derive(Debug)]
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
                UndoStep::Interface(Box::new(SetInterface::plan(document, id, to, library)))
            }
        };
        if !step.is_noop() {
            step.apply(document);
            steps.push(step);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
