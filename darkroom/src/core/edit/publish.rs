//! Pure document-to-graph-library promotion and publication operations.

use scenarium::Library;
use scenarium::{Graph, GraphId, GraphLink};
use scenarium::{NodeId, NodeKind, NodeSearch};

use crate::core::document::{Document, GraphRef, ItemRef};
use crate::core::graph_library::GraphLibrary;

#[derive(Clone, Copy, Debug)]
pub(crate) enum GraphPublicationTarget {
    ActiveGraph,
    LocalNode { target: GraphRef, node_id: NodeId },
}

pub(crate) fn publish_graph_to_library(
    document: &mut Document,
    graph_library: &mut GraphLibrary,
    target: GraphPublicationTarget,
) -> bool {
    match target {
        GraphPublicationTarget::ActiveGraph => promote_to_graph_library(document, graph_library),
        GraphPublicationTarget::LocalNode { target, node_id } => {
            publish_local_graph_to_library(document, graph_library, target, node_id)
        }
    }
}

/// Publish `node_id`'s local graph into `graph_library`
/// (no disk write — the caller persists on success). Returns `false`
/// when the node isn't a local graph instance in `target`.
///
/// When its `origin` still resolves, that shared graph is updated in
/// place so existing instances keep their link.
/// Otherwise a fresh-id copy joins the graph library and the local graph's
/// `origin` is re-pointed at it, so a later publish
/// updates rather than re-adds. The `origin` write is lineage metadata,
/// deliberately *not* routed through undo.
pub(crate) fn publish_local_graph_to_library(
    document: &mut Document,
    graph_library: &mut GraphLibrary,
    target: GraphRef,
    node_id: NodeId,
) -> bool {
    let Some(source) = (|| {
        let scope = document.scope(target)?;
        let NodeKind::Graph(GraphLink::Local(local_id)) =
            scope.graph.find(&node_id, NodeSearch::TopLevel)?.kind
        else {
            return None;
        };
        let local = scope.graph.graphs.get(&local_id)?;
        let existing_lib = local
            .origin
            .filter(|id| graph_library.graphs.contains_key(id));
        Some(PublishSource {
            local_id,
            graph: local.fresh_copy(),
            existing_id: existing_lib,
        })
    })() else {
        return false;
    };

    let new_origin = source.existing_id.unwrap_or_else(GraphId::unique);
    graph_library.graphs.insert(new_origin, source.graph);
    set_origin(document, target, source.local_id, new_origin);
    true
}

#[derive(Debug)]
struct PublishSource {
    local_id: GraphId,
    graph: Graph,
    existing_id: Option<GraphId>,
}

/// Promote the active/selected graph into `graph_library` as a new entry
/// (no disk write — the caller persists on success). Returns `false`
/// when nothing resolves. On success the source local graph's `origin` is
/// re-pointed at the new library entry, so it tracks its lineage.
pub(crate) fn promote_to_graph_library(
    document: &mut Document,
    graph_library: &mut GraphLibrary,
) -> bool {
    let Some(promotable) = resolve_promotable(document) else {
        return false;
    };
    let Some((graph, relink)) = (|| -> Option<(Graph, Option<RelinkLocal>)> {
        Some(match promotable {
            Promotable::Node {
                graph: holder,
                link: GraphLink::Local(graph_id),
            } => {
                let graph = document.graph_for(holder)?.graphs.get(&graph_id)?.clone();
                (graph, Some(RelinkLocal { holder, graph_id }))
            }
            Promotable::Node {
                link: GraphLink::Shared(graph_id),
                ..
            } => (graph_library.graphs.get(&graph_id)?.clone(), None),
            Promotable::OpenTab { id } => (
                document.graph.graphs.get(&id)?.clone(),
                Some(RelinkLocal {
                    holder: GraphRef::Main,
                    graph_id: id,
                }),
            ),
        })
    })() else {
        return false;
    };
    let lib_id = GraphId::unique();
    graph_library.graphs.insert(lib_id, graph.fresh_copy());
    if let Some(relink) = relink {
        set_origin(document, relink.holder, relink.graph_id, lib_id);
    }
    true
}

/// Point the local graph at the library entry `origin`. Lineage metadata —
/// not routed through undo.
fn set_origin(document: &mut Document, holder: GraphRef, graph_id: GraphId, origin: GraphId) {
    if let Some(graph) = document.graph_mut(holder)
        && let Some(nested) = graph.graphs.get_mut(&graph_id)
    {
        nested.origin = Some(origin);
    }
}

#[derive(Debug)]
struct RelinkLocal {
    holder: GraphRef,
    graph_id: GraphId,
}

#[derive(Debug)]
enum Promotable {
    Node { graph: GraphRef, link: GraphLink },
    OpenTab { id: GraphId },
}

/// Resolve the graph targeted by export.
pub(crate) fn graph_template_to_export<'a>(
    document: &'a Document,
    library: &'a Library,
) -> Option<&'a Graph> {
    match resolve_promotable(document)? {
        Promotable::Node { graph, link } => document.graph_for(graph)?.resolve_graph(link, library),
        Promotable::OpenTab { id } => document.graph.graphs.get(&id),
    }
}

fn resolve_promotable(document: &Document) -> Option<Promotable> {
    // Export/promote act on the active graph; a non-graph tab has none.
    let target = document.active_target()?;
    let graph = document.graph_for(target)?;
    if let Some(view) = document.view(target) {
        for key in &view.selected {
            let ItemRef::Node(nid) = key else {
                continue;
            };
            if let Some(node) = graph.find(nid, NodeSearch::TopLevel)
                && let NodeKind::Graph(link) = node.kind
            {
                return Some(Promotable::Node {
                    graph: target,
                    link,
                });
            }
        }
    }
    match target {
        GraphRef::Local(id) if document.graph.graphs.contains_key(&id) => {
            Some(Promotable::OpenTab { id })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use scenarium::{FuncId, Graph, GraphId, GraphLink, Node, NodeId, NodeKind};

    use crate::core::document::{Document, GraphRef, ItemRef};
    use crate::core::edit::publish::{promote_to_graph_library, publish_local_graph_to_library};
    use crate::core::graph_library::GraphLibrary;

    #[derive(Debug)]
    struct LocalInstance {
        graph_id: GraphId,
        node_id: NodeId,
    }

    fn add_local_instance(doc: &mut Document, graph: Graph) -> LocalInstance {
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

    fn graph(name: &str, origin: Option<GraphId>) -> Graph {
        let mut graph = Graph::new(name);
        graph.origin = origin;
        graph
    }

    #[test]
    fn publish_updates_linked_library_def_in_place() {
        let lib_id = GraphId::unique();
        let mut graph_library = GraphLibrary::default();
        graph_library.graphs.insert(lib_id, Graph::new("Old"));

        // Local copy linked to that library graph, with diverged content.
        let mut doc = Document::default();
        let local = add_local_instance(&mut doc, graph("New", Some(lib_id)));

        assert!(publish_local_graph_to_library(
            &mut doc,
            &mut graph_library,
            GraphRef::Main,
            local.node_id
        ));
        assert_eq!(
            graph_library.graphs.len(),
            1,
            "update in place — no new library entry"
        );
        assert_eq!(
            graph_library.graphs.get(&lib_id).unwrap().name,
            "New",
            "library graph took the local graph's content"
        );
        assert_eq!(
            doc.graph.graphs.get(&local.graph_id).unwrap().origin,
            Some(lib_id),
            "lineage preserved"
        );
    }

    #[test]
    fn publish_without_origin_creates_entry_and_links_it() {
        let mut graph_library = GraphLibrary::default();
        let mut doc = Document::default();
        let local = add_local_instance(&mut doc, graph("Standalone", None));

        assert!(publish_local_graph_to_library(
            &mut doc,
            &mut graph_library,
            GraphRef::Main,
            local.node_id
        ));
        assert_eq!(
            graph_library.graphs.len(),
            1,
            "a new graph-library entry was added"
        );
        let linked = doc
            .graph
            .graphs
            .get(&local.graph_id)
            .unwrap()
            .origin
            .expect("local graph linked to the new entry");
        assert!(
            graph_library.graphs.contains_key(&linked),
            "origin points at the freshly-created library graph"
        );
    }

    #[test]
    fn promote_links_source_local_def_to_new_library_entry() {
        let mut graph_library = GraphLibrary::default();
        let mut doc = Document::default();
        // A local graph instance (no library lineage yet), selected
        // so `promote_source` resolves it from the active graph.
        let local = add_local_instance(&mut doc, graph("Widget", None));
        doc.main_view.selected.insert(ItemRef::Node(local.node_id));

        assert!(promote_to_graph_library(&mut doc, &mut graph_library));
        assert_eq!(
            graph_library.graphs.len(),
            1,
            "a new graph-library entry is added"
        );
        let owner = doc
            .graph
            .graphs
            .get(&local.graph_id)
            .unwrap()
            .origin
            .expect("source local graph now carries an origin");
        assert!(
            graph_library.graphs.contains_key(&owner),
            "origin points at the freshly-promoted library entry"
        );
    }

    #[test]
    fn promote_copies_a_selected_shared_graph() {
        let source_id = GraphId::unique();
        let source = Graph::new("Shared");
        let mut graph_library = GraphLibrary::default();
        graph_library.graphs.insert(source_id, source.clone());
        let mut doc = Document::default();
        let node_id = doc
            .graph
            .add(Node::graph_instance(&source, GraphLink::Shared(source_id)));
        doc.main_view.selected.insert(ItemRef::Node(node_id));

        assert!(promote_to_graph_library(&mut doc, &mut graph_library));
        assert_eq!(graph_library.graphs.len(), 2);
        assert_eq!(graph_library.graphs.get(&source_id).unwrap(), &source);
        assert_eq!(
            graph_library
                .graphs
                .values()
                .filter(|graph| graph.name == "Shared")
                .count(),
            2,
            "promotion creates a new template instead of changing its source"
        );
    }

    #[test]
    fn promote_with_nothing_selected_is_a_noop() {
        let mut graph_library = GraphLibrary::default();
        let mut doc = Document::default();
        assert!(!promote_to_graph_library(&mut doc, &mut graph_library));
        assert!(graph_library.graphs.is_empty());
    }

    #[test]
    fn publish_non_graph_node_is_a_noop() {
        let mut graph_library = GraphLibrary::default();
        let mut doc = Document::default();
        let node = Node::new(NodeKind::Func(FuncId::unique()));
        let node_id = doc.graph.add(node);

        assert!(!publish_local_graph_to_library(
            &mut doc,
            &mut graph_library,
            GraphRef::Main,
            node_id
        ));
        assert!(graph_library.graphs.is_empty(), "nothing published");
    }
}
