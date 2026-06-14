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
use scenarium::data::{FsPathConfig, StaticValue};
use scenarium::function::FuncLib;
use scenarium::graph::{Binding, NodeKind};
use scenarium::prelude::{NodeId, SubgraphDef, SubgraphId};
use scenarium::subgraph::SubgraphRef;

use crate::core::document::{Document, GraphRef};
use crate::core::edit::intent::Intent;
use crate::core::io::library;
use crate::core::io::persistence;
use crate::gui::app::App;
use crate::gui::app::editor::Editor;
use crate::gui::dialogs;
use crate::gui::menu_bar::MenuCommand;
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
            MenuCommand::LoadTheme => {
                if let Some(path) = dialogs::pick_theme_open() {
                    self.load_theme(ui, &path);
                }
            }
            MenuCommand::ExportTheme => {
                if let Some(path) = dialogs::pick_theme_save() {
                    dialogs::export_theme(&self.theme, &path);
                }
            }
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
        }
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
        self.editor.apply_edit(Intent::SetInput {
            node_id,
            input_idx: port_idx,
            to: Binding::Const(value),
        });
    }

    /// Export a subgraph def to a file (its interior `Graph` carries any
    /// nested subgraph defs along). A selected subgraph-instance node
    /// wins; otherwise, when the active tab is itself a subgraph, that
    /// open subgraph is exported. No-op when neither resolves.
    fn export_active_subgraph(&mut self) {
        let func_lib = self.engine.func_lib.load();
        let Some(def) = subgraph_to_export(&self.editor.document, &func_lib) else {
            eprintln!("subgraph export: no subgraph selected or open");
            return;
        };
        if let Some(path) = dialogs::pick_save_path(self.current_path.as_deref()) {
            persistence::export_subgraph(def, &path);
        }
    }

    /// Publish a copy of the active/selected subgraph into the shared
    /// library (the runtime `FuncLib`) and persist it, so it can be
    /// instanced as `Linked` anywhere. A fresh-id copy (fresh interior
    /// ids too) joins the library, and the source local def's `origin`
    /// is pointed at it so it now tracks that library entry (a later
    /// Publish updates in place). The `origin` write is lineage metadata,
    /// not routed through undo. No-op when neither a subgraph instance
    /// nor an open subgraph resolves.
    fn promote_active_subgraph(&mut self) {
        if promote_to_library(&mut self.editor.document, &self.engine.func_lib) {
            library::save_library(self.engine.func_lib.load().subgraphs.iter());
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
        let target = self.editor.document.active_target();
        if publish_local_def(
            &mut self.editor.document,
            &self.engine.func_lib,
            target,
            node_id,
        ) {
            library::save_library(self.engine.func_lib.load().subgraphs.iter());
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

    pub(crate) fn load_document(&mut self, path: &Path) {
        let Some(doc) = persistence::load_document(path) else {
            return;
        };
        // Fresh editor around the loaded doc — see `new_document` for why
        // a wholesale reset (rather than poking individual fields) is right.
        self.editor = Editor::new(doc);
        self.set_document_path(Some(path.to_path_buf()));
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
    /// next launch reopens this document.
    fn set_document_path(&mut self, path: Option<PathBuf>) {
        self.current_path = path.clone();
        self.config.document_path = path;
        self.config.save();
    }

    /// Load a theme picked from the dialog and apply it to the live
    /// session. Not persisted: the next launch always restores the
    /// preference recorded in [`AppConfig::theme`] (set via the Theme
    /// menu), regardless of any file loaded in this session.
    fn load_theme(&mut self, ui: &mut Ui, picked: &Path) {
        if self.load_theme_file(picked) {
            ui.theme = self.theme.palantir_theme.clone();
        }
    }

    /// Apply a theme `.toml` from `path` into `self.theme`. Returns
    /// whether it succeeded; on failure leaves the current theme
    /// untouched. Shared by startup restore and menu load.
    pub(crate) fn load_theme_file(&mut self, path: &Path) -> bool {
        match dialogs::load_theme(path) {
            Some(theme) => {
                self.theme = theme;
                true
            }
            None => false,
        }
    }
}

/// Publish/refresh `node_id`'s local subgraph def into `func_lib`
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
/// can be unit-tested against a bare `Document` + `FuncLib`.
fn publish_local_def(
    document: &mut Document,
    func_lib: &ArcSwap<FuncLib>,
    target: GraphRef,
    node_id: NodeId,
) -> bool {
    // Load a snapshot and resolve read-only first (so a non-subgraph node
    // returns before the `Arc::make_mut` clone). `fresh_copy` already gives
    // fresh interior ids + `origin: None` — the shape a library def wants;
    // we keep or override its id below.
    let mut lib = func_lib.load_full();
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
    func_lib.store(lib);
    set_origin(document, target, local_id, new_origin);
    true
}

/// Promote the active/selected subgraph into `func_lib` as a new entry
/// (no disk write — the caller persists on success). Returns `false`
/// when nothing resolves. On success the source local def's `origin` is
/// re-pointed at the new library entry, so it tracks its lineage. Free
/// fn so it's unit-testable against a bare `Document` + `FuncLib`.
fn promote_to_library(document: &mut Document, func_lib: &ArcSwap<FuncLib>) -> bool {
    let mut lib = func_lib.load_full();
    let Some(source) = promote_source(document, &lib) else {
        return false;
    };
    let published = source.def.fresh_copy();
    let lib_id = published.id;
    Arc::make_mut(&mut lib).add_subgraph(published);
    func_lib.store(lib);
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
/// graph's own `Local` table or the shared `FuncLib` for `Linked`), else
/// the currently open subgraph when inside one. `None` when neither
/// resolves. Pure resolution over the document — kept here with its only
/// callers rather than on the `Document` model.
fn subgraph_to_export<'a>(
    document: &'a Document,
    func_lib: &'a FuncLib,
) -> Option<&'a SubgraphDef> {
    match resolve_promotable(document, func_lib)? {
        Promotable::Node { graph, sref } => document.graph_for(graph)?.resolve_def(sref, func_lib),
        Promotable::OpenTab { id } => document.graph.subgraphs.by_key(&id),
    }
}

/// Like [`subgraph_to_export`], but for promoting into the library:
/// returns an owned copy of the resolved def plus, when the source is a
/// `Local` def in this document, where to re-link its `origin` after the
/// library entry is created. `None` (no relink) for a `Linked` source —
/// there's no in-document def to own it.
fn promote_source(document: &Document, func_lib: &FuncLib) -> Option<PromoteSource> {
    let (def, relink) = match resolve_promotable(document, func_lib)? {
        Promotable::Node { graph, sref } => {
            let def = document
                .graph_for(graph)?
                .resolve_def(sref, func_lib)?
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
fn resolve_promotable(document: &Document, func_lib: &FuncLib) -> Option<Promotable> {
    let target = document.active_target();
    let graph = document.graph_for(target)?;
    if let Some(view) = document.view(target) {
        for nid in &view.selected_nodes {
            if let Some(node) = graph.by_id(nid)
                && let NodeKind::Subgraph(sref) = node.kind
                && graph.resolve_def(sref, func_lib).is_some()
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
        SubgraphDef {
            id: SubgraphId::unique(),
            name: name.into(),
            origin,
            ..Default::default()
        }
    }

    #[test]
    fn publish_updates_linked_library_def_in_place() {
        let lib_id = SubgraphId::unique();
        let mut func_lib = FuncLib::default();
        func_lib.add_subgraph(SubgraphDef {
            id: lib_id,
            name: "Old".into(),
            ..Default::default()
        });
        let func_lib = ArcSwap::from_pointee(func_lib);

        // Local copy linked to that library def, with diverged content.
        let mut doc = Document::default();
        let local = def("New", Some(lib_id));
        let local_id = local.id;
        let node_id = add_local_instance(&mut doc, local);

        assert!(publish_local_def(
            &mut doc,
            &func_lib,
            GraphRef::Main,
            node_id
        ));
        assert_eq!(
            func_lib.load().subgraphs.len(),
            1,
            "update in place — no new library entry"
        );
        assert_eq!(
            func_lib.load().subgraph_by_id(&lib_id).unwrap().name,
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
        let func_lib = ArcSwap::from_pointee(FuncLib::default());
        let mut doc = Document::default();
        let local = def("Standalone", None);
        let local_id = local.id;
        let node_id = add_local_instance(&mut doc, local);

        assert!(publish_local_def(
            &mut doc,
            &func_lib,
            GraphRef::Main,
            node_id
        ));
        assert_eq!(
            func_lib.load().subgraphs.len(),
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
            func_lib.load().subgraph_by_id(&linked).is_some(),
            "origin points at the freshly-created library def"
        );
    }

    #[test]
    fn promote_links_source_local_def_to_new_library_entry() {
        let func_lib = ArcSwap::from_pointee(FuncLib::default());
        let mut doc = Document::default();
        // A local subgraph instance (no library lineage yet), selected
        // so `promote_source` resolves it from the active graph.
        let local = def("Widget", None);
        let local_id = local.id;
        let node_id = add_local_instance(&mut doc, local);
        doc.main_view.selected_nodes.insert(node_id);

        assert!(promote_to_library(&mut doc, &func_lib));
        assert_eq!(
            func_lib.load().subgraphs.len(),
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
            func_lib.load().subgraph_by_id(&owner).is_some(),
            "origin points at the freshly-promoted library entry"
        );
    }

    #[test]
    fn promote_with_nothing_selected_is_a_noop() {
        let func_lib = ArcSwap::from_pointee(FuncLib::default());
        let mut doc = Document::default();
        assert!(!promote_to_library(&mut doc, &func_lib));
        assert_eq!(func_lib.load().subgraphs.len(), 0);
    }

    #[test]
    fn publish_non_subgraph_node_is_a_noop() {
        use scenarium::prelude::FuncId;
        let func_lib = ArcSwap::from_pointee(FuncLib::default());
        let mut doc = Document::default();
        let node = Node::new(scenarium::graph::NodeKind::Func(FuncId::unique()));
        let node_id = node.id;
        doc.graph.add(node);

        assert!(!publish_local_def(
            &mut doc,
            &func_lib,
            GraphRef::Main,
            node_id
        ));
        assert_eq!(func_lib.load().subgraphs.len(), 0, "nothing published");
    }
}
