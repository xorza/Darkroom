//! Graph template export and publication operations.

use scenarium::Library;
use scenarium::{GraphDef, GraphId, GraphLink};
use scenarium::{NodeId, NodeKind, NodeSearch};

use crate::core::document::{Document, GraphRef};

/// A publication read out of the document but not yet committed anywhere.
/// Resolving and committing are separate so the library file is written
/// *first*: only once the entry is durably on disk does the caller adopt
/// the merged library and [`link_origin`] the local graph to it. A publish
/// that fails to persist then leaves nothing behind — no phantom library
/// entry that works all session and vanishes on restart, and no document
/// lineage pointing at one.
#[derive(Debug)]
pub(crate) struct Publication {
    /// The local graph being published — the one whose lineage the commit
    /// re-points.
    pub(crate) local_id: GraphId,
    /// The library entry this local graph came from, if any. Whether it
    /// still exists is decided against the file at commit time, not here.
    pub(crate) origin: Option<GraphId>,
    pub(crate) graph: GraphDef,
}

/// Read the publication `node_id` names, or `None` when it isn't a local
/// graph instance. Pure — the document is untouched until [`link_origin`].
pub(crate) fn resolve_publication(
    document: &Document,
    target: GraphRef,
    node_id: NodeId,
) -> Option<Publication> {
    let scope = document.scope(target)?;
    let NodeKind::Graph(GraphLink::Local(local_id)) =
        scope.graph.find(node_id, NodeSearch::TopLevel)?.kind
    else {
        return None;
    };
    let local = scope.graph.graphs.get(&local_id)?;
    Some(Publication {
        local_id,
        origin: local.interface.origin,
        graph: local.clone_mapped(),
    })
}

/// Point the local graph at the library entry it was committed to. Lineage
/// metadata — not routed through undo.
pub(crate) fn link_origin(
    document: &mut Document,
    parent: GraphRef,
    graph_id: GraphId,
    origin: GraphId,
) {
    if let Some(graph) = document.graph_mut(parent)
        && let Some(nested) = graph.graphs.get_mut(&graph_id)
    {
        nested.interface.origin = Some(origin);
    }
}

#[derive(Debug)]
enum ExportTarget {
    Node { graph: GraphRef, link: GraphLink },
    OpenTab { id: GraphId },
}

/// Resolve the graph targeted by export.
pub(crate) fn graph_template_to_export<'a>(
    document: &'a Document,
    library: &'a Library,
) -> Option<&'a GraphDef> {
    match resolve_export_target(document)? {
        ExportTarget::Node { graph, link } => {
            document.graph_for(graph)?.resolve_graph(link, library)
        }
        ExportTarget::OpenTab { id } => document.graph.find_graph(id),
    }
}

fn resolve_export_target(document: &Document) -> Option<ExportTarget> {
    let target = document.focused_target()?;
    let graph = document.graph_for(target)?;
    if let Some(view) = document.view(target) {
        for key in &view.selected {
            let nid = key;
            if let Some(node) = graph.find(*nid, NodeSearch::TopLevel)
                && let NodeKind::Graph(link) = node.kind
            {
                return Some(ExportTarget::Node {
                    graph: target,
                    link,
                });
            }
        }
    }
    match target {
        // `graph_for(target)` above already proved the def resolves.
        GraphRef::Local(id) => Some(ExportTarget::OpenTab { id }),
        GraphRef::Main => None,
    }
}

#[cfg(test)]
mod tests {
    use scenarium::{FuncId, GraphDef, GraphId, GraphLink, Node, NodeId, NodeKind};

    use crate::core::document::{Document, GraphRef};
    use crate::core::edit::publish::{link_origin, resolve_publication};

    #[derive(Debug)]
    struct LocalInstance {
        graph_id: GraphId,
        node_id: NodeId,
    }

    fn add_local_instance(doc: &mut Document, graph: GraphDef) -> LocalInstance {
        let graph_id = GraphId::unique();
        doc.graph.insert_graph(graph_id, graph);
        let node = Node::graph_instance(
            doc.graph.graphs.get(&graph_id).unwrap(),
            GraphLink::Local(graph_id),
        );
        LocalInstance {
            graph_id,
            node_id: doc.graph.add(node),
        }
    }

    fn graph(name: &str, origin: Option<GraphId>) -> GraphDef {
        let mut graph = GraphDef::new(name);
        graph.interface.origin = origin;
        graph
    }

    fn origin_of(doc: &Document, graph_id: GraphId) -> Option<GraphId> {
        doc.graph.graphs.get(&graph_id).unwrap().interface.origin
    }

    #[test]
    fn resolving_reads_the_local_graph_without_touching_the_document() {
        // Resolve is the read half: it reports what would be published and
        // the lineage to reuse, but commits nothing — the library file is
        // written before `link_origin` moves anything in the document.
        let lib_id = GraphId::unique();
        let mut doc = Document::default();
        let linked = add_local_instance(&mut doc, graph("Linked", Some(lib_id)));
        let standalone = add_local_instance(&mut doc, graph("Standalone", None));

        let publication = resolve_publication(&doc, GraphRef::Main, linked.node_id)
            .expect("a local graph instance resolves");
        assert_eq!(publication.local_id, linked.graph_id);
        assert_eq!(
            publication.origin,
            Some(lib_id),
            "a linked graph carries its lineage forward for the commit to reuse"
        );
        assert_eq!(publication.graph.interface.name, "Linked");
        assert_eq!(
            origin_of(&doc, linked.graph_id),
            Some(lib_id),
            "resolving wrote nothing"
        );

        let publication = resolve_publication(&doc, GraphRef::Main, standalone.node_id)
            .expect("an unlinked local graph resolves too");
        assert_eq!(
            publication.origin, None,
            "no lineage means the commit mints a fresh id"
        );

        // Only a local graph instance publishes.
        let plain = doc.graph.add(Node::new(NodeKind::Func(FuncId::unique())));
        assert!(resolve_publication(&doc, GraphRef::Main, plain).is_none());
        assert!(resolve_publication(&doc, GraphRef::Main, NodeId::unique()).is_none());
    }

    #[test]
    fn linking_points_the_local_graph_at_its_committed_entry() {
        let mut doc = Document::default();
        let local = add_local_instance(&mut doc, graph("Standalone", None));
        let committed = GraphId::unique();

        link_origin(&mut doc, GraphRef::Main, local.graph_id, committed);
        assert_eq!(origin_of(&doc, local.graph_id), Some(committed));

        // A graph that has since vanished is a no-op, not a panic.
        doc.graph.graphs.remove(&local.graph_id);
        link_origin(&mut doc, GraphRef::Main, local.graph_id, committed);
    }
}
