//! Make Unique: the picked instances given a definition of their own.
//!
//! - **Make Unique is shallow.** The copy has a fresh id and the same nodes under the same ids,
//!   and its inner instances name the definitions they named, as Blender's single-user copy
//!   duplicates one data-block. A deep copy is Make Unique again, one level in.
//! - **It lowers to ordinary requests**: the copy coming in, then each picked instance pointed at
//!   it, in one batch, so one history entry.
//! - **Duplicate and paste make another instance; Make Unique gives the picked ones a definition
//!   of their own.** As Blender, Grasshopper ("the two instances of the cluster will be entangled")
//!   and ComfyUI, whose subgraphs are "linked rather than duplicated" ([release
//!   notes](https://blog.comfy.org/p/subgraph-official-release)). Refused: a copy by default, as
//!   Nuke's Group, where ten copies of one pipeline are ten graphs to fix.

use scenarium::{Compound, CompoundId, GraphId, NodeId, TypeNames};

use crate::core::document::Document;
use crate::core::edit::compound_intent::{CompoundIntent, NewCompound};
use crate::core::edit::document_queue::DocumentQueue;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::node_property::NodeProperty;

/// What a Make Unique raises: the copy, and the instances of `graph` it points at it.
#[derive(Debug)]
pub(crate) struct MakeUnique {
    pub(crate) copy: NewCompound,
    graph: GraphId,
    instances: Vec<NodeId>,
}

impl MakeUnique {
    /// Plan giving `instances`, instances of `graph` that all place one compound, a copy of it;
    /// `None` when they place none, or more than one.
    pub(crate) fn plan(document: &Document, graph: GraphId, instances: &[NodeId]) -> Option<Self> {
        let id = Self::placed(document, graph, instances)?;
        let original = document.graphs.compound(id)?;
        let mut compound = Compound::from(original);
        compound.decl.name = document.free_compound_name(&original.decl.name);
        let mut side = document.sides[&id].clone();
        side.through = None;
        Some(Self {
            copy: NewCompound {
                id: CompoundId::unique(),
                compound,
                side,
                type_names: TypeNames::default(),
            },
            graph,
            instances: instances.to_vec(),
        })
    }

    /// The one compound `instances`, nodes of `graph`, all place, or `None` when they place none
    /// or more than one, or `graph` takes no edit, as its re-pointing would be refused and leave
    /// the copy behind: whether a Make Unique of them is offered, answered without the copy a
    /// plan makes.
    pub(crate) fn placed(
        document: &Document,
        graph: GraphId,
        instances: &[NodeId],
    ) -> Option<CompoundId> {
        if document.read_only(graph).is_some() {
            return None;
        }
        let held = document.graphs.graph(graph);
        let mut placed = instances
            .iter()
            .map(|&node_id| held.find(node_id)?.compound());
        let id = placed.next()??;
        placed.all(|other| other == Some(id)).then_some(id)
    }

    /// Queue the copy and the re-pointing, in the order they land: one batch, so one entry.
    pub(crate) fn queue(self, queue: &mut DocumentQueue) {
        let to = self.copy.id;
        queue.push_compound(CompoundIntent::Add(Box::new(self.copy)));
        queue.extend_graph(
            self.graph,
            self.instances
                .into_iter()
                .map(|node_id| GraphIntent::SetNodeProperty {
                    node_id,
                    to: NodeProperty::Compound(to),
                }),
        );
    }
}

#[cfg(test)]
mod tests {
    use scenarium::{Compound, CompoundId, GraphId, Interface};

    use crate::core::document::internals::{DocumentFixture, insert_compound};
    use crate::core::edit::document_queue::DocumentQueue;
    use crate::core::edit::make_unique::MakeUnique;
    use crate::core::open_document::OpenDocument;

    /// Making one of two instances unique gives it a copy under a fresh id, holding the same
    /// nodes under the same ids, named apart, while the other keeps the original; one undo takes
    /// both back. Instances of two compounds make nothing.
    #[test]
    fn make_unique_gives_the_picked_instance_a_copy_of_its_own() {
        let mut fixture = DocumentFixture::default();
        let [id, other] = [1, 2].map(CompoundId::from_u128);
        let mut compound = Compound::new("Denoise", Interface::default());
        let inside = compound.graph.add(fixture.plain_node());
        insert_compound(&mut fixture.document, id, compound);
        insert_compound(
            &mut fixture.document,
            other,
            Compound::new("Other", Interface::default()),
        );
        let [picked, kept] = [(); 2].map(|()| fixture.add_instance(id));
        let stranger = fixture.add_instance(other);
        let DocumentFixture { document, library } = fixture;
        assert!(MakeUnique::plan(&document, GraphId::Main, &[picked, stranger]).is_none());
        let mut open = OpenDocument::over(document);
        let plan = MakeUnique::plan(open.document(), GraphId::Main, &[picked]).unwrap();
        let copy = plan.copy.id;
        let mut queue = DocumentQueue::default();
        plan.queue(&mut queue);
        let _ = open.drain_requests(&mut queue, &library);

        let document = open.document();
        let main = document.graphs.main();
        assert_eq!(main.find(picked).unwrap().compound(), Some(copy));
        assert_eq!(main.find(kept).unwrap().compound(), Some(id));
        let made = document.graphs.compound(copy).unwrap();
        assert_eq!(made.decl.name, "Denoise 2");
        assert!(
            made.graph.find(inside).is_some(),
            "the same nodes under the same ids"
        );
        assert_ne!(
            document.graphs.digest(copy),
            document.graphs.digest(id),
            "named apart"
        );
        let _ = open.undo();
        let document = open.document();
        assert!(document.graphs.compound(copy).is_none());
        assert_eq!(
            document.graphs.main().find(picked).unwrap().compound(),
            Some(id)
        );
    }
}
