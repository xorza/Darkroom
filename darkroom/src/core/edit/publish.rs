//! Publishing local subgraph defs into the shared [`Library`]: the pure
//! document↔library resolution + mutation behind the GUI's
//! export / promote / publish commands. Operates only on [`Document`] +
//! [`Library`] (no GUI, dialogs, or persistence), so it's unit-testable
//! against bare types; the thin orchestration (file dialogs, disk writes,
//! marking the document dirty) stays in `gui::app::commands`.

use std::sync::Arc;

use arc_swap::ArcSwap;
use scenarium::graph::subgraph::{SubgraphDef, SubgraphId, SubgraphRef};
use scenarium::graph::{NodeId, NodeKind};
use scenarium::library::Library;

use crate::core::document::{Document, GraphRef};

/// Publish/refresh `node_id`'s local subgraph def into `library`
/// (no disk write — the caller persists on success). Returns `false`
/// when the node isn't a local subgraph instance in `target`.
///
/// When the local def is linked to a still-present library def
/// (`origin` resolves), that def is **updated in place** (its id is
/// reused so `add_subgraph` overwrites it and `Linked` instances pick
/// up the new content). Otherwise a fresh-id copy joins the library and
/// the local def's `origin` is re-pointed at it, so a later publish
/// updates rather than re-adds. The `origin` write is lineage metadata,
/// deliberately *not* routed through undo.
pub(crate) fn publish_local_def(
    document: &mut Document,
    library: &ArcSwap<Library>,
    target: GraphRef,
    node_id: NodeId,
) -> bool {
    // Load a snapshot and resolve read-only first (so a non-subgraph node
    // returns before the `Arc::make_mut` clone). `fresh_copy` already gives
    // fresh interior ids + `origin: None` — the shape a library def wants;
    // we keep or override its id below.
    let mut lib = library.load_full();
    let Some((local_id, mut published, existing_lib)) = (|| {
        let scope = document.scope(target)?;
        let NodeKind::Subgraph(SubgraphRef::Local(local_id)) = scope.graph.by_id(&node_id)?.kind
        else {
            return None;
        };
        let local = scope.graph.subgraphs.by_key(&local_id)?;
        let existing_lib = local.origin.filter(|id| lib.subgraph_by_id(id).is_some());
        Some((local_id, local.fresh_copy(), existing_lib))
    })() else {
        return false;
    };

    let new_origin = match existing_lib {
        Some(lib_id) => {
            published.id = lib_id;
            Arc::make_mut(&mut lib).add_subgraph(published);
            lib_id
        }
        None => {
            let new_id = published.id;
            Arc::make_mut(&mut lib).add_subgraph(published);
            new_id
        }
    };
    // Swap the grown copy in so the worker and any running scripts pick it
    // up on their next load.
    library.store(lib);
    set_origin(document, target, local_id, new_origin);
    true
}

/// Promote the active/selected subgraph into `library` as a new entry
/// (no disk write — the caller persists on success). Returns `false`
/// when nothing resolves. On success the source local def's `origin` is
/// re-pointed at the new library entry, so it tracks its lineage.
pub(crate) fn promote_to_library(document: &mut Document, library: &ArcSwap<Library>) -> bool {
    let mut lib = library.load_full();
    let Some(source) = promote_source(document, &lib) else {
        return false;
    };
    let published = source.def.fresh_copy();
    let lib_id = published.id;
    Arc::make_mut(&mut lib).add_subgraph(published);
    library.store(lib);
    if let Some(relink) = source.relink {
        set_origin(document, relink.holder, relink.def_id, lib_id);
    }
    true
}

/// Point the local subgraph def `def_id` (in `holder`'s graph) at the
/// library entry `origin`. No-op if the def is gone. Lineage metadata —
/// not routed through undo.
fn set_origin(document: &mut Document, holder: GraphRef, def_id: SubgraphId, origin: SubgraphId) {
    if let Some(graph) = document.graph_mut(holder)
        && let Some(def) = graph.subgraphs.by_key_mut(&def_id)
    {
        def.origin = Some(origin);
    }
}

/// A subgraph resolved for promotion into the library: the def to
/// publish (owned copy) plus where to re-link its `origin` afterward.
struct PromoteSource {
    def: SubgraphDef,
    /// Where the source local def lives, so we can point its `origin` at
    /// the freshly-created library entry. `None` when the source is a
    /// library (`Linked`) def — nothing in the document to re-link.
    relink: Option<RelinkLocal>,
}

/// Locates a local subgraph def for an `origin` re-link: the graph
/// whose local table holds it, plus the def's id.
struct RelinkLocal {
    holder: GraphRef,
    def_id: SubgraphId,
}

/// Internal resolution result shared by export + promote.
enum Promotable {
    /// First selected subgraph-instance node in the active graph.
    Node { graph: GraphRef, sref: SubgraphRef },
    /// The open subgraph interior (its def lives in the root table).
    OpenTab { id: SubgraphId },
}

/// Resolve which subgraph def an export targets: the first selected
/// subgraph-instance node in the active graph (resolved against that
/// graph's own `Local` table or the shared `Library` for `Linked`), else
/// the currently open subgraph when inside one. `None` when neither
/// resolves. Pure resolution over the document — kept here with its only
/// callers rather than on the `Document` model.
pub(crate) fn subgraph_to_export<'a>(
    document: &'a Document,
    library: &'a Library,
) -> Option<&'a SubgraphDef> {
    match resolve_promotable(document, library)? {
        Promotable::Node { graph, sref } => document.graph_for(graph)?.resolve_def(sref, library),
        Promotable::OpenTab { id } => document.graph.subgraphs.by_key(&id),
    }
}

/// Like [`subgraph_to_export`], but for promoting into the library:
/// returns an owned copy of the resolved def plus, when the source is a
/// `Local` def in this document, where to re-link its `origin` after the
/// library entry is created. `None` (no relink) for a `Linked` source —
/// there's no in-document def to own it.
fn promote_source(document: &Document, library: &Library) -> Option<PromoteSource> {
    let (def, relink) = match resolve_promotable(document, library)? {
        Promotable::Node { graph, sref } => {
            let def = document
                .graph_for(graph)?
                .resolve_def(sref, library)?
                .clone();
            let relink = match sref {
                SubgraphRef::Local(id) => Some(RelinkLocal {
                    holder: graph,
                    def_id: id,
                }),
                SubgraphRef::Linked(_) => None,
            };
            (def, relink)
        }
        // An open subgraph tab is always a `Local` def living in the root
        // table, regardless of which interior is shown.
        Promotable::OpenTab { id } => (
            document.graph.subgraphs.by_key(&id)?.clone(),
            Some(RelinkLocal {
                holder: GraphRef::Main,
                def_id: id,
            }),
        ),
    };
    Some(PromoteSource { def, relink })
}

/// Shared resolution for export / promote: the first selected
/// subgraph-instance node in the active graph (whose def resolves), else
/// the open subgraph interior. `None` when neither applies.
fn resolve_promotable(document: &Document, library: &Library) -> Option<Promotable> {
    // Export/promote act on the active graph; a non-graph tab has none.
    let target = document.active_target()?;
    let graph = document.graph_for(target)?;
    if let Some(view) = document.view(target) {
        for nid in &view.selected_nodes {
            if let Some(node) = graph.by_id(nid)
                && let NodeKind::Subgraph(sref) = node.kind
                && graph.resolve_def(sref, library).is_some()
            {
                return Some(Promotable::Node {
                    graph: target,
                    sref,
                });
            }
        }
    }
    match target {
        GraphRef::Local(id) if document.graph.subgraphs.by_key(&id).is_some() => {
            Some(Promotable::OpenTab { id })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenarium::graph::Node;
    use scenarium::graph::subgraph::SubgraphDef;

    /// Add a local subgraph def `def` to `doc`'s root graph plus an
    /// instance node referencing it; return the node id.
    fn add_local_instance(doc: &mut Document, def: SubgraphDef) -> NodeId {
        let local_id = def.id;
        doc.graph.subgraphs.add(def);
        let node = Node::subgraph_instance(
            doc.graph.subgraphs.by_key(&local_id).unwrap(),
            SubgraphRef::Local(local_id),
        );
        let node_id = node.id;
        doc.graph.add(node);
        node_id
    }

    fn def(name: &str, origin: Option<SubgraphId>) -> SubgraphDef {
        let mut def = SubgraphDef::new(SubgraphId::unique(), name);
        def.origin = origin;
        def
    }

    #[test]
    fn publish_updates_linked_library_def_in_place() {
        let lib_id = SubgraphId::unique();
        let mut library = Library::default();
        library.add_subgraph(SubgraphDef::new(lib_id, "Old"));
        let library = ArcSwap::from_pointee(library);

        // Local copy linked to that library def, with diverged content.
        let mut doc = Document::default();
        let local = def("New", Some(lib_id));
        let local_id = local.id;
        let node_id = add_local_instance(&mut doc, local);

        assert!(publish_local_def(
            &mut doc,
            &library,
            GraphRef::Main,
            node_id
        ));
        assert_eq!(
            library.load().subgraphs.len(),
            1,
            "update in place — no new library entry"
        );
        assert_eq!(
            library.load().subgraph_by_id(&lib_id).unwrap().name,
            "New",
            "library def took the local def's content"
        );
        assert_eq!(
            doc.graph.subgraphs.by_key(&local_id).unwrap().origin,
            Some(lib_id),
            "lineage preserved"
        );
    }

    #[test]
    fn publish_without_origin_creates_entry_and_links_it() {
        let library = ArcSwap::from_pointee(Library::default());
        let mut doc = Document::default();
        let local = def("Standalone", None);
        let local_id = local.id;
        let node_id = add_local_instance(&mut doc, local);

        assert!(publish_local_def(
            &mut doc,
            &library,
            GraphRef::Main,
            node_id
        ));
        assert_eq!(
            library.load().subgraphs.len(),
            1,
            "a new library entry was added"
        );
        let linked = doc
            .graph
            .subgraphs
            .by_key(&local_id)
            .unwrap()
            .origin
            .expect("local def linked to the new entry");
        assert!(
            library.load().subgraph_by_id(&linked).is_some(),
            "origin points at the freshly-created library def"
        );
    }

    #[test]
    fn promote_links_source_local_def_to_new_library_entry() {
        let library = ArcSwap::from_pointee(Library::default());
        let mut doc = Document::default();
        // A local subgraph instance (no library lineage yet), selected
        // so `promote_source` resolves it from the active graph.
        let local = def("Widget", None);
        let local_id = local.id;
        let node_id = add_local_instance(&mut doc, local);
        doc.main_view.selected_nodes.insert(node_id);

        assert!(promote_to_library(&mut doc, &library));
        assert_eq!(
            library.load().subgraphs.len(),
            1,
            "a new library entry is added"
        );
        let owner = doc
            .graph
            .subgraphs
            .by_key(&local_id)
            .unwrap()
            .origin
            .expect("source local def now carries an origin");
        assert!(
            library.load().subgraph_by_id(&owner).is_some(),
            "origin points at the freshly-promoted library entry"
        );
    }

    #[test]
    fn promote_with_nothing_selected_is_a_noop() {
        let library = ArcSwap::from_pointee(Library::default());
        let mut doc = Document::default();
        assert!(!promote_to_library(&mut doc, &library));
        assert_eq!(library.load().subgraphs.len(), 0);
    }

    #[test]
    fn publish_non_subgraph_node_is_a_noop() {
        use scenarium::node::function::FuncId;
        let library = ArcSwap::from_pointee(Library::default());
        let mut doc = Document::default();
        let node = Node::new(scenarium::graph::NodeKind::Func(FuncId::unique()));
        let node_id = node.id;
        doc.graph.add(node);

        assert!(!publish_local_def(
            &mut doc,
            &library,
            GraphRef::Main,
            node_id
        ));
        assert_eq!(library.load().subgraphs.len(), 0, "nothing published");
    }
}
