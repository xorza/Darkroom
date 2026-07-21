//! Publishing local graphs into the shared [`Library`]: the pure
//! document↔library resolution + mutation behind the GUI's
//! export / promote / publish commands. Operates only on [`Document`] +
//! [`Library`] (no GUI, dialogs, or persistence), so it's unit-testable
//! against bare types. The thin orchestration (file dialogs, marking the
//! document dirty) stays in `gui::app::commands`, which runs the mutators
//! through `RuntimeHost::edit_library` — the one path that reports persistence
//! outcomes and propagates the grown library to its downstream copies.

use scenarium::Library;
use scenarium::{Graph, GraphId, GraphLink};
use scenarium::{NodeId, NodeKind, NodeSearch};

use crate::core::document::{Document, GraphRef, ItemRef};

/// Publish `node_id`'s local graph into `library`
/// (no disk write — the caller persists on success). Returns `false`
/// when the node isn't a local graph instance in `target`.
///
/// When its `origin` still resolves, that shared graph is updated in
/// place so existing instances keep their link.
/// Otherwise a fresh-id copy joins the library and the local graph's
/// `origin` is re-pointed at it, so a later publish
/// updates rather than re-adds. The `origin` write is lineage metadata,
/// deliberately *not* routed through undo.
pub(crate) fn publish_local_graph(
    document: &mut Document,
    library: &mut Library,
    target: GraphRef,
    node_id: NodeId,
) -> bool {
    let Some((local_id, published, existing_lib)) = (|| {
        let scope = document.scope(target)?;
        let NodeKind::Graph(GraphLink::Local(local_id)) =
            scope.graph.find(&node_id, NodeSearch::TopLevel)?.kind
        else {
            return None;
        };
        let local = scope.graph.graphs.get(&local_id)?;
        let existing_lib = local.origin.filter(|id| library.graph_by_id(id).is_some());
        Some((local_id, local.fresh_copy(), existing_lib))
    })() else {
        return false;
    };

    let new_origin = existing_lib.unwrap_or_else(GraphId::unique);
    library.insert_graph(new_origin, published);
    set_origin(document, target, local_id, new_origin);
    true
}

/// Promote the active/selected graph into `library` as a new entry
/// (no disk write — the caller persists on success). Returns `false`
/// when nothing resolves. On success the source local graph's `origin` is
/// re-pointed at the new library entry, so it tracks its lineage.
pub(crate) fn promote_to_library(document: &mut Document, library: &mut Library) -> bool {
    let Some(source) = promote_source(document, library) else {
        return false;
    };
    let published = source.graph.fresh_copy();
    let lib_id = GraphId::unique();
    library.insert_graph(lib_id, published);
    if let Some(relink) = source.relink {
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
struct PromoteSource {
    graph: Graph,
    /// Where the source local graph lives, so we can point its `origin` at
    /// the freshly-created library entry. `None` when the source is a
    /// shared graph — nothing in the document to re-link.
    relink: Option<RelinkLocal>,
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
pub(crate) fn graph_to_export<'a>(
    document: &'a Document,
    library: &'a Library,
) -> Option<&'a Graph> {
    match resolve_promotable(document, library)? {
        Promotable::Node { graph, link } => document.graph_for(graph)?.resolve_graph(link, library),
        Promotable::OpenTab { id } => document.graph.graphs.get(&id),
    }
}

/// Like [`graph_to_export`], but for promoting into the library:
/// returns an owned copy and, for a local source, where to update lineage.
fn promote_source(document: &Document, library: &Library) -> Option<PromoteSource> {
    let (graph, relink) = match resolve_promotable(document, library)? {
        Promotable::Node { graph, link } => {
            let nested = document
                .graph_for(graph)?
                .resolve_graph(link, library)?
                .clone();
            let relink = match link {
                GraphLink::Local(id) => Some(RelinkLocal {
                    holder: graph,
                    graph_id: id,
                }),
                GraphLink::Shared(_) => None,
            };
            (nested, relink)
        }
        Promotable::OpenTab { id } => (
            document.graph.graphs.get(&id)?.clone(),
            Some(RelinkLocal {
                holder: GraphRef::Main,
                graph_id: id,
            }),
        ),
    };
    Some(PromoteSource { graph, relink })
}

/// Shared resolution for export and promote.
fn resolve_promotable(document: &Document, library: &Library) -> Option<Promotable> {
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
                && graph.resolve_graph(link, library).is_some()
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
    use super::*;
    use scenarium::Graph;
    use scenarium::Node;

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
        let mut library = Library::default();
        library.insert_graph(lib_id, Graph::new("Old"));

        // Local copy linked to that library graph, with diverged content.
        let mut doc = Document::default();
        let local = add_local_instance(&mut doc, graph("New", Some(lib_id)));

        assert!(publish_local_graph(
            &mut doc,
            &mut library,
            GraphRef::Main,
            local.node_id
        ));
        assert_eq!(
            library.graphs.len(),
            1,
            "update in place — no new library entry"
        );
        assert_eq!(
            library.graph_by_id(&lib_id).unwrap().name,
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
        let mut library = Library::default();
        let mut doc = Document::default();
        let local = add_local_instance(&mut doc, graph("Standalone", None));

        assert!(publish_local_graph(
            &mut doc,
            &mut library,
            GraphRef::Main,
            local.node_id
        ));
        assert_eq!(library.graphs.len(), 1, "a new library entry was added");
        let linked = doc
            .graph
            .graphs
            .get(&local.graph_id)
            .unwrap()
            .origin
            .expect("local graph linked to the new entry");
        assert!(
            library.graph_by_id(&linked).is_some(),
            "origin points at the freshly-created library graph"
        );
    }

    #[test]
    fn promote_links_source_local_def_to_new_library_entry() {
        let mut library = Library::default();
        let mut doc = Document::default();
        // A local graph instance (no library lineage yet), selected
        // so `promote_source` resolves it from the active graph.
        let local = add_local_instance(&mut doc, graph("Widget", None));
        doc.main_view.selected.insert(ItemRef::Node(local.node_id));

        assert!(promote_to_library(&mut doc, &mut library));
        assert_eq!(library.graphs.len(), 1, "a new library entry is added");
        let owner = doc
            .graph
            .graphs
            .get(&local.graph_id)
            .unwrap()
            .origin
            .expect("source local graph now carries an origin");
        assert!(
            library.graph_by_id(&owner).is_some(),
            "origin points at the freshly-promoted library entry"
        );
    }

    #[test]
    fn promote_with_nothing_selected_is_a_noop() {
        let mut library = Library::default();
        let mut doc = Document::default();
        assert!(!promote_to_library(&mut doc, &mut library));
        assert_eq!(library.graphs.len(), 0);
    }

    #[test]
    fn publish_non_graph_node_is_a_noop() {
        use scenarium::FuncId;
        let mut library = Library::default();
        let mut doc = Document::default();
        let node = Node::new(scenarium::NodeKind::Func(FuncId::unique()));
        let node_id = doc.graph.add(node);

        assert!(!publish_local_graph(
            &mut doc,
            &mut library,
            GraphRef::Main,
            node_id
        ));
        assert_eq!(library.graphs.len(), 0, "nothing published");
    }
}
