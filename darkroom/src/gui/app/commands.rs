//! Menu-command side effects: the file/theme/subgraph load-save flows
//! `App` runs *outside* the record pass (after the frame's record +
//! drain), so the blocking file dialog holds no frame borrows. Kept
//! apart from the per-frame pipeline in `app.rs` — these only touch
//! `Document`/`Theme`/`AppConfig` + persistence, never the gesture or
//! scene state.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_swap::ArcSwap;
use palantir::Ui;
use scenarium::data::{FsPathConfig, FsPathMode, StaticValue};
use scenarium::graph::{Binding, NodeKind};
use scenarium::library::Library;
use scenarium::prelude::{NodeId, SubgraphDef, SubgraphId};
use scenarium::subgraph::SubgraphRef;

use crate::core::document::{Document, GraphRef};
use crate::core::edit::intent::Intent;
use crate::core::io::library;
use crate::core::io::persistence;
use crate::gui::app::App;
use crate::gui::app::editor::Editor;
use crate::gui::dialogs;
use crate::gui::menu_bar::{MenuCommand, MlModelKind};
use crate::gui::theme::Theme;

impl App {
    pub(crate) fn handle_menu_command(&mut self, ui: &mut Ui, command: MenuCommand) {
        match command {
            MenuCommand::NewDocument => self.new_document(),
            MenuCommand::LoadDocument => {
                if let Some(path) = dialogs::pick_open_path(self.current_path.as_deref()) {
                    self.load_document(&path);
                }
            }
            MenuCommand::SaveDocument => self.save_current(),
            MenuCommand::SaveDocumentAs => self.save_document_as(),
            MenuCommand::SetTheme(choice) => {
                // Resolve the choice to a concrete palette (`System`
                // queries the OS), push the matching palantir palette onto
                // the Ui, and persist the preference so the next launch
                // restores it.
                self.theme = Theme::from_preset(choice.resolve());
                ui.theme = self.theme.palantir_theme.clone();
                self.config.theme = choice;
                self.config.save();
            }
            MenuCommand::ExportSubgraph => self.export_active_subgraph(),
            MenuCommand::ImportSubgraph => self.import_subgraph(),
            MenuCommand::PromoteSubgraph => self.promote_active_subgraph(),
            MenuCommand::PublishNodeSubgraph { node_id } => self.publish_node_subgraph(node_id),
            MenuCommand::PickInputPath {
                node_id,
                port_idx,
                config,
            } => self.pick_input_path(node_id, port_idx, config),
            MenuCommand::Run => self.run_graph(),
            MenuCommand::CancelRun => self.engine.cancel_run(),
            MenuCommand::OpenConfig => {
                let library = self.engine.library.load();
                self.editor.open_config(&library);
            }
            MenuCommand::PickMlModel(kind) => self.pick_ml_model(kind),
            MenuCommand::SetMlModelPath { kind, path } => self.set_ml_model_path(kind, path),
            MenuCommand::SetLoadLastDocument(on) => {
                self.config.load_last_document = on;
                self.config.save();
            }
        }
    }

    /// Open an ONNX file dialog for one of the ML model paths and, on a
    /// pick, persist it and republish the paths to lens so the next
    /// `ml_denoise` / `remove_stars` run uses the new model. Runs outside
    /// the record (blocking dialog), like the other file ops.
    fn pick_ml_model(&mut self, kind: MlModelKind) {
        let filter =
            FsPathConfig::with_extensions(FsPathMode::ExistingFile, vec!["onnx".to_string()]);
        if let Some(path) = dialogs::pick_path(&filter) {
            self.set_ml_model_path(kind, path);
        }
    }

    /// Record `path` for `kind`, persist the config, and republish the paths
    /// to lens so the next node run uses it. Shared by the Browse dialog and
    /// the Config tab's editable path field.
    fn set_ml_model_path(&mut self, kind: MlModelKind, path: PathBuf) {
        match kind {
            MlModelKind::Denoise => self.config.ml_models.denoise = path,
            MlModelKind::StarRemoval => self.config.ml_models.star_removal = path,
        }
        self.config.save();
        self.config.apply_ml_model_paths();
    }

    /// Open a file dialog for a node's `FsPath` const input and, if the
    /// user picks one, apply the chosen path as a `SetInput` edit. Runs
    /// outside the record (blocking dialog), so it goes through
    /// `Editor::apply_edit` rather than the frame's intent drain.
    fn pick_input_path(&mut self, node_id: NodeId, port_idx: usize, config: Arc<FsPathConfig>) {
        let Some(path) = dialogs::pick_path(&config) else {
            return;
        };
        let value = StaticValue::FsPath(path.to_string_lossy().into_owned());
        let library = self.engine.library.load();
        self.editor.apply_edit(
            Intent::SetInput {
                node_id,
                input_idx: port_idx,
                to: Binding::Const(value),
            },
            &library,
        );
    }

    /// Export a subgraph def to a file (its interior `Graph` carries any
    /// nested subgraph defs along). A selected subgraph-instance node
    /// wins; otherwise, when the active tab is itself a subgraph, that
    /// open subgraph is exported. No-op when neither resolves.
    fn export_active_subgraph(&mut self) {
        let library = self.engine.library.load();
        let Some(def) = subgraph_to_export(&self.editor.document, &library) else {
            eprintln!("subgraph export: no subgraph selected or open");
            return;
        };
        if let Some(path) = dialogs::pick_save_path(self.current_path.as_deref()) {
            persistence::export_subgraph(def, &path);
        }
    }

    /// Publish a copy of the active/selected subgraph into the shared
    /// library (the runtime `Library`) and persist it, so it can be
    /// instanced as `Linked` anywhere. A fresh-id copy (fresh interior
    /// ids too) joins the library, and the source local def's `origin`
    /// is pointed at it so it now tracks that library entry (a later
    /// Publish updates in place). The `origin` write is lineage metadata,
    /// not routed through undo. No-op when neither a subgraph instance
    /// nor an open subgraph resolves.
    fn promote_active_subgraph(&mut self) {
        if promote_to_library(&mut self.editor.document, &self.engine.library) {
            library::save_library(self.engine.library.load().subgraphs.iter());
        } else {
            eprintln!("subgraph promote: no subgraph selected or open");
        }
    }

    /// Publish a node's local subgraph def to the shared library (the
    /// S-badge "Publish" action). When the local def is linked to a
    /// library def (`origin` resolves), that library def is **updated in
    /// place** (same id, so existing `Linked` instances pick up the new
    /// content). Otherwise a fresh-id copy joins the library and the
    /// local def's `origin` is pointed at it, so a later Publish updates
    /// rather than re-adds. Non-undoable (library + disk only); the node
    /// resolves against the active graph.
    fn publish_node_subgraph(&mut self, node_id: NodeId) {
        // The S-badge that raises this only exists on the canvas, so a
        // graph tab is always active here; bail otherwise.
        let Some(target) = self.editor.document.active_target() else {
            return;
        };
        if publish_local_def(
            &mut self.editor.document,
            &self.engine.library,
            target,
            node_id,
        ) {
            library::save_library(self.engine.library.load().subgraphs.iter());
        } else {
            eprintln!("subgraph publish: node is not a local subgraph");
        }
    }

    /// Import a subgraph def from a file as a local def in the current
    /// document. The import is a copy with a fresh id; nothing is
    /// instantiated and the undo stack is untouched (existing history
    /// references no imported def, so it stays valid).
    fn import_subgraph(&mut self) {
        let Some(path) = dialogs::pick_open_path(self.current_path.as_deref()) else {
            return;
        };
        if let Some(def) = persistence::import_subgraph(&path) {
            self.editor.import_subgraph(def);
        }
    }

    /// Replace the document with an empty one. A fresh [`Editor`] resets
    /// all derived/transient state in one move: empty undo history
    /// (restoring the old doc via Cmd-Z would replay nodes from intent
    /// history that no longer matches the live tree), forced reconcile +
    /// scene rebuild, dropped gesture state, and cleared run results.
    fn new_document(&mut self) {
        self.editor = Editor::new(Document::default());
        self.set_document_path(None);
    }

    /// Load `path` into a fresh editor. Returns whether it loaded — `false`
    /// when the file is missing/corrupt (startup uses this to drop a stale
    /// `document_path`; the menu-load path ignores it, leaving the open doc).
    pub(crate) fn load_document(&mut self, path: &Path) -> bool {
        let Some(doc) = persistence::load_document(path) else {
            return false;
        };
        // Fresh editor around the loaded doc — see `new_document` for why
        // a wholesale reset (rather than poking individual fields) is right.
        self.editor = Editor::new(doc);
        self.set_document_path(Some(path.to_path_buf()));
        true
    }

    /// Cmd+S: overwrite the current file if there is one, else fall
    /// back to Save As (first save of a fresh document).
    fn save_current(&mut self) {
        match self.current_path.clone() {
            Some(path) => self.save_document(&path),
            None => self.save_document_as(),
        }
    }

    /// Cmd+Shift+S / "Save As…": always prompt for a destination.
    fn save_document_as(&mut self) {
        if let Some(path) = dialogs::pick_save_path(self.current_path.as_deref()) {
            self.save_document(&path);
        }
    }

    fn save_document(&mut self, path: &Path) {
        if persistence::save_document(&self.editor.document, path) {
            self.set_document_path(Some(path.to_path_buf()));
        }
    }

    /// Record `path` as both the dialog-anchor `current_path` and the
    /// persisted `config.document_path`, then write the config so the
    /// next launch reopens this document. Also repoints the worker's disk
    /// cache at the document's project-local store (or memory-only when the
    /// path is cleared / never saved).
    fn set_document_path(&mut self, path: Option<PathBuf>) {
        self.current_path = path.clone();
        self.engine.set_document_cache(self.current_path.as_deref());
        self.config.document_path = path;
        self.config.save();
    }
}

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
/// deliberately *not* routed through undo. Free fn (not a method) so it
/// can be unit-tested against a bare `Document` + `Library`.
fn publish_local_def(
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
/// re-pointed at the new library entry, so it tracks its lineage. Free
/// fn so it's unit-testable against a bare `Document` + `Library`.
fn promote_to_library(document: &mut Document, library: &ArcSwap<Library>) -> bool {
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
pub(crate) struct PromoteSource {
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
fn subgraph_to_export<'a>(document: &'a Document, library: &'a Library) -> Option<&'a SubgraphDef> {
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
    use scenarium::prelude::SubgraphDef;

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
        use scenarium::prelude::FuncId;
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
