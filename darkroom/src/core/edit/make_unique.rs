//! Make Unique: the picked instances given a definition of their own.
//!
//! - **Make Unique is shallow.** The copy has a fresh id and the same nodes under the same ids,
//!   and its inner instances name the definitions they named, as Blender's single-user copy
//!   duplicates one data-block. A deep copy is Make Unique again, one level in.
//! - **It lowers to ordinary requests**: the copy coming in, then each picked instance pointed at
//!   it, in one batch, so one history entry.

use scenarium::{GraphId, NodeId, SubgraphId};

use crate::core::document::Document;
use crate::core::edit::document_queue::DocumentQueue;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::node_property::NodeProperty;
use crate::core::edit::subgraph_intent::{NewSubgraph, SubgraphIntent};

/// What a Make Unique raises: the copy, and the instances of `graph` it points at it.
#[derive(Debug)]
pub(crate) struct MakeUnique {
    pub(crate) copy: NewSubgraph,
    graph: GraphId,
    instances: Vec<NodeId>,
}

impl MakeUnique {
    /// Plan giving `instances`, instances of `graph` that all place one subgraph, a copy of it;
    /// `None` when they place none, or more than one.
    pub(crate) fn plan(document: &Document, graph: GraphId, instances: &[NodeId]) -> Option<Self> {
        let id = Self::placed(document, graph, instances)?;
        let original = document.graphs.subgraph(id)?;
        let mut subgraph = original.clone();
        subgraph.name = document.free_subgraph_name(&original.name);
        let mut side = document.sides[&id].clone();
        side.through = None;
        Some(Self {
            copy: NewSubgraph {
                id: SubgraphId::unique(),
                subgraph,
                side,
            },
            graph,
            instances: instances.to_vec(),
        })
    }

    /// The one subgraph `instances`, nodes of `graph`, all place, or `None` when they place none
    /// or more than one: whether a Make Unique of them is offered, answered without the copy a
    /// plan makes.
    pub(crate) fn placed(
        document: &Document,
        graph: GraphId,
        instances: &[NodeId],
    ) -> Option<SubgraphId> {
        let held = document.graphs.graph(graph);
        let mut placed = instances
            .iter()
            .map(|&node_id| held.find(node_id)?.subgraph());
        let id = placed.next()??;
        placed.all(|other| other == Some(id)).then_some(id)
    }

    /// Queue the copy and the re-pointing, in the order they land: one batch, so one entry.
    pub(crate) fn queue(self, queue: &mut DocumentQueue) {
        let to = self.copy.id;
        queue.push_subgraph(SubgraphIntent::Add(Box::new(self.copy)));
        queue.extend_graph(
            self.graph,
            self.instances
                .into_iter()
                .map(|node_id| GraphIntent::SetNodeProperty {
                    node_id,
                    to: NodeProperty::Subgraph(to),
                }),
        );
    }
}

#[cfg(test)]
mod tests {
    use scenarium::{GraphId, Interface, Subgraph, SubgraphId};

    use crate::core::document::internals::{DocumentFixture, insert_subgraph};
    use crate::core::edit::document_queue::DocumentQueue;
    use crate::core::edit::make_unique::MakeUnique;
    use crate::core::open_document::OpenDocument;

    /// Making one of two instances unique gives it a copy under a fresh id, holding the same
    /// nodes under the same ids, named apart, while the other keeps the original; one undo takes
    /// both back. Instances of two subgraphs make nothing.
    #[test]
    fn make_unique_gives_the_picked_instance_a_copy_of_its_own() {
        let mut fixture = DocumentFixture::default();
        let [id, other] = [1, 2].map(SubgraphId::from_u128);
        let mut subgraph = Subgraph::new("Denoise", Interface::default());
        let inside = subgraph.graph.add(fixture.plain_node());
        insert_subgraph(&mut fixture.document, id, subgraph);
        insert_subgraph(
            &mut fixture.document,
            other,
            Subgraph::new("Other", Interface::default()),
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
        assert_eq!(main.find(picked).unwrap().subgraph(), Some(copy));
        assert_eq!(main.find(kept).unwrap().subgraph(), Some(id));
        let made = document.graphs.subgraph(copy).unwrap();
        assert_eq!(made.name, "Denoise 2");
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
        assert!(document.graphs.subgraph(copy).is_none());
        assert_eq!(
            document.graphs.main().find(picked).unwrap().subgraph(),
            Some(id)
        );
    }
}
