//! Graph template export and publication operations.

use scenarium::Library;
use scenarium::{Graph, GraphId, GraphLink};
use scenarium::{NodeId, NodeKind, NodeSearch};

use crate::core::document::{Document, GraphRef, ItemRef};
use crate::core::graph_library::GraphLibrary;

pub(crate) fn publish_graph(
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
            .definition
            .as_ref()?
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

/// Point the local graph at the library entry `origin`. Lineage metadata —
/// not routed through undo.
fn set_origin(document: &mut Document, holder: GraphRef, graph_id: GraphId, origin: GraphId) {
    if let Some(graph) = document.graph_mut(holder)
        && let Some(nested) = graph.graphs.get_mut(&graph_id)
    {
        nested.definition.as_mut().unwrap().origin = Some(origin);
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
) -> Option<&'a Graph> {
    match resolve_export_target(document)? {
        ExportTarget::Node { graph, link } => {
            document.graph_for(graph)?.resolve_graph(link, library)
        }
        ExportTarget::OpenTab { id } => document.graph.graphs.get(&id),
    }
}

fn resolve_export_target(document: &Document) -> Option<ExportTarget> {
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
                return Some(ExportTarget::Node {
                    graph: target,
                    link,
                });
            }
        }
    }
    match target {
        GraphRef::Local(id) if document.graph.graphs.contains_key(&id) => {
            Some(ExportTarget::OpenTab { id })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use scenarium::{FuncId, Graph, GraphId, GraphLink, Node, NodeId, NodeKind};

    use crate::core::document::{Document, GraphRef};
    use crate::core::edit::publish::publish_graph;
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
        graph.definition.as_mut().unwrap().origin = origin;
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

        assert!(publish_graph(
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
            graph_library
                .graphs
                .get(&lib_id)
                .unwrap()
                .definition
                .as_ref()
                .unwrap()
                .name,
            "New",
            "library graph took the local graph's content"
        );
        assert_eq!(
            doc.graph
                .graphs
                .get(&local.graph_id)
                .unwrap()
                .definition
                .as_ref()
                .unwrap()
                .origin,
            Some(lib_id),
            "lineage preserved"
        );
    }

    #[test]
    fn publish_without_origin_creates_entry_and_links_it() {
        let mut graph_library = GraphLibrary::default();
        let mut doc = Document::default();
        let local = add_local_instance(&mut doc, graph("Standalone", None));

        assert!(publish_graph(
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
            .definition
            .as_ref()
            .unwrap()
            .origin
            .expect("local graph linked to the new entry");
        assert!(
            graph_library.graphs.contains_key(&linked),
            "origin points at the freshly-created library graph"
        );
    }

    #[test]
    fn publish_non_graph_node_is_a_noop() {
        let mut graph_library = GraphLibrary::default();
        let mut doc = Document::default();
        let node = Node::new(NodeKind::Func(FuncId::unique()));
        let node_id = doc.graph.add(node);

        assert!(!publish_graph(
            &mut doc,
            &mut graph_library,
            GraphRef::Main,
            node_id
        ));
        assert!(graph_library.graphs.is_empty(), "nothing published");
    }
}
