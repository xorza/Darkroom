use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use glam::Vec2;
use lens::ImageFuncLib;
use palantir::{HostHandle, Shortcut, Ui};
use scenarium::elements::basic_funclib::BasicFuncLib;
use scenarium::elements::worker_events_funclib::WorkerEventsFuncLib;
use scenarium::graph::NodeKind;
use scenarium::prelude::{FuncLib, SubgraphDef, SubgraphId};
use scenarium::testing::{TestFuncHooks, test_func_lib};

use crate::action_stack::ActionStack;
use crate::config::AppConfig;
use crate::document::{Document, GraphRef, GraphView};
use crate::gui::UiAction;
use crate::gui::main_window::MainWindow;
use crate::gui::menu_bar::MenuCommand;
use crate::gui::tab_bar::TabLabel;
use crate::intent::{self, Intent, apply_step, build_step, requires_relayout};
use crate::persistence;
use crate::sample_graph::sample_graph;
use crate::scene::Scene;
use crate::theme::Theme;

const UNDO_SHORTCUT: Shortcut = Shortcut::ctrl('Z');
const REDO_SHORTCUT: Shortcut = Shortcut::ctrl_shift('Z');
const NEW_SHORTCUT: Shortcut = Shortcut::ctrl('N');
const OPEN_SHORTCUT: Shortcut = Shortcut::ctrl('O');
const SAVE_SHORTCUT: Shortcut = Shortcut::ctrl('S');
const SAVE_AS_SHORTCUT: Shortcut = Shortcut::ctrl_shift('S');
const RESET_ZOOM_SHORTCUT: Shortcut = Shortcut::ctrl('0');

/// Byte budget for the undo history's packed buffer (~1 MiB). Bounds
/// memory rather than entry count — a single large edit can't be
/// undone away, but the oldest entries drop once the buffer overflows.
const UNDO_HISTORY_BYTES: usize = 1 << 20;

/// Shared per-frame context threaded down the UI tree. Holds borrows
/// of state owned higher up so child subtrees don't take a growing
/// fan-out of `&` parameters. Currently just the active [`Theme`];
/// future per-frame shared state (selection, debug toggles, etc.)
/// lives here too.
#[derive(Copy, Clone, Debug)]
pub struct AppContext<'a> {
    pub theme: &'a Theme,
    pub func_lib: &'a FuncLib,
}

#[derive(Debug)]
pub struct App {
    pub document: Document,
    pub func_lib: FuncLib,
    pub scene: Scene,
    /// Which graph `scene` last reflected. A mismatch with the active
    /// target means the tab changed: drop transient gesture state
    /// (`reset_transient`) and request a relayout. `None` forces that on
    /// the next frame (set by document replacement). It does *not* gate
    /// the rebuild itself — the projection is rebuilt every frame
    /// regardless (see `frame`).
    scene_target: Option<GraphRef>,
    /// Set by `drain_intents` whenever it applies a step; consumed by the
    /// pre-record rebuild so the record sees doc edits the pre-record
    /// drain made (drag, connection commit). Only meaningful in the
    /// window between the unconditional pre-prepass rebuild (which clears
    /// it) and the pre-record rebuild.
    scene_dirty: bool,
    pub main_window: MainWindow,
    /// Per-frame scratch buffer of pending mutations. Cleared at the
    /// top of every `frame`, filled by prepass/record/shortcut
    /// handling, and fully drained before `frame` returns — it carries
    /// no state across frames. Kept as a field only to reuse the
    /// allocation; not part of `App`'s observable state.
    intents: Vec<Intent>,
    /// Per-frame scratch buffer of view-state requests (open/activate/
    /// close tab) raised during record. Drained each frame; carries no
    /// cross-frame state — kept only to reuse the allocation.
    actions: Vec<UiAction>,
    pub action_stack: ActionStack,
    pub theme: Theme,
    pub host_handle: HostHandle,
    /// Last successfully loaded/saved file path. `Save…` and `Load…`
    /// preopen the dialog at this directory so a session that touches
    /// many files in the same folder doesn't re-navigate each time.
    pub current_path: Option<PathBuf>,
    /// Persisted session state (active theme name + last document).
    /// Written on every doc/theme change so the next launch reopens
    /// where the user left off.
    pub config: AppConfig,
}

impl palantir::App for App {
    /// Build the app before the first frame: assemble the func lib +
    /// seed document, then restore persisted config (saved theme +
    /// last document) and push the resolved palantir theme onto `Ui`.
    /// Restore failures degrade silently to defaults — a missing or
    /// corrupt config, or a deleted document, must not block launch.
    fn new(ui: &mut Ui, handle: HostHandle) -> Self {
        let mut document: Document = sample_graph().into();
        document
            .main_view
            .auto_layout(&document.graph, 220.0, 110.0, Vec2::new(40.0, 40.0));
        let mut func_lib = FuncLib::default();
        func_lib.merge(test_func_lib(TestFuncHooks::default()));
        func_lib.merge(BasicFuncLib::default());
        func_lib.merge(WorkerEventsFuncLib::default());
        func_lib.merge(ImageFuncLib::default());
        let mut app = Self {
            document,
            func_lib,
            scene: Scene::default(),
            scene_target: None,
            scene_dirty: false,
            main_window: MainWindow::default(),
            intents: Vec::new(),
            actions: Vec::new(),
            action_stack: ActionStack::new(UNDO_HISTORY_BYTES),
            theme: Theme::default(),
            host_handle: handle,
            current_path: None,
            config: AppConfig::default(),
        };
        app.config = AppConfig::load();
        if let Some(name) = app.config.theme_name.clone() {
            app.load_theme_file(&AppConfig::theme_path(&name));
        }
        if let Some(path) = app.config.document_path.clone() {
            app.load_document(&path);
        }
        // Resolved theme (default, or whatever the config restored)
        // onto the Ui so palantir widgets paint correctly frame 1.
        ui.theme = app.theme.palantir_theme.clone();
        app
    }

    fn frame(&mut self, ui: &mut Ui) {
        // ui.debug_overlay.damage_rect = true;

        self.intents.clear();
        self.actions.clear();

        // ── Navigation phase ─────────────────────────────────────────
        // Settle the active graph entirely from frame-top inputs
        // (keyboard undo/redo + last-frame click responses). `navigate`
        // reads *last* frame's `scene` to resolve tab/chip clicks, so it
        // must run before this frame's rebuild. After it, `target` is
        // fixed for the rest of the frame.
        let mut relayout = self.navigate(ui);
        let target = self.document.active_target();
        relayout |= self.sync_target(target);

        // Rebuild the projection for this frame, after the navigation
        // phase has fully settled the document — so prepass and
        // `PortFrame` never read a stale graph (the old "rebuild only on
        // tab switch" path let an undo/redo leave them a frame behind).
        // Unconditional: `Scene` re-interns port names into palantir's
        // per-frame text arena, which clears each `Ui::frame`, so the
        // projection must be regenerated every frame regardless.
        self.rebuild_scene(target);
        self.scene_dirty = false;

        // ── Edit phase ───────────────────────────────────────────────
        // Prepass emits input-derived graph mutations (drag, pan/zoom,
        // connection commit) drained *before* the record so Pass A sees
        // the settled doc. It reads everything off `Scene`, so it takes
        // neither the func lib nor the full `AppContext`.
        self.main_window.prepass(ui, &self.scene, &mut self.intents);
        relayout |= self.drain_intents(target);
        relayout |= self.apply_canvas_shortcuts(ui, target);

        let command_from_shortcut = self.menu_shortcut(ui);

        // Record. Rebuild again only if the pre-record drain actually
        // changed the doc (drag, connection commit) — an idle frame or a
        // bare tab switch leaves `scene_dirty` false and skips it, so the
        // tab-switch frame rebuilds once, not twice.
        if self.scene_dirty {
            self.rebuild_scene(target);
            self.scene_dirty = false;
        }
        let ctx = AppContext {
            theme: &self.theme,
            func_lib: &self.func_lib,
        };
        let tab_labels = self.tab_labels();
        let active = self.document.active;
        let host = self.host_handle.clone();
        let command = self
            .main_window
            .frame(
                ui,
                &ctx,
                &mut self.scene,
                Some(&host),
                &tab_labels,
                active,
                &mut self.intents,
            )
            .or(command_from_shortcut);

        // Post-record drain — graph edits the record surfaced (node
        // select, cache toggle, const edit). Navigation is fully settled
        // in the navigation phase, so nothing here moves the active tab.
        relayout |= self.drain_intents(target);
        if relayout {
            ui.request_relayout();
        }

        // Menu side effects run last so the blocking file dialog opens
        // after the frame's record + drain. Loading replaces the
        // document/theme wholesale, so always relayout afterward
        // regardless of what `drain_intents` decided.
        if let Some(command) = command {
            self.handle_menu_command(ui, command);
            ui.request_relayout();
        }
    }
}

impl App {
    /// Settle which graph is active for this frame, from inputs all
    /// available before the record: keyboard undo/redo (which can replay
    /// a `SwitchTab`) and tab/subgraph-open clicks read from *last*
    /// frame's responses. Returns whether a relayout is needed.
    ///
    /// Done up front so the edit pipeline runs against a fixed target and
    /// a switched-to graph records in the same present's Pass A.
    fn navigate(&mut self, ui: &mut Ui) -> bool {
        let mut relayout = self.apply_undo_redo(ui);
        // Surface tab/open clicks from last frame's responses. `scene`
        // still holds the last-rendered graph here — exactly the one
        // whose chips were clicked.
        let tab_count = self.document.tabs.len();
        self.main_window
            .scan_navigation(ui, &self.scene, tab_count, &mut self.actions);
        // Open mutates the tab list directly; activate/close queue
        // undoable `SwitchTab` / `CloseTab` intents — drain them (both
        // steps are graph-agnostic, so the target passed here doesn't
        // matter).
        relayout |= self.apply_view_actions();
        relayout |= self.drain_intents(self.document.active_target());
        // A closed/deleted target can't be active; fall back to Main.
        self.ensure_valid_active();
        relayout
    }

    /// Note a possible active-graph change: when `target` differs from
    /// what `scene` last reflected, drop transient gesture state (so a
    /// drag started on one graph can't bleed into another) and request a
    /// relayout. Keeps `PortFrame`'s offset cache, so a graph shown again
    /// resolves its port centers immediately. The rebuild itself is the
    /// caller's unconditional one — this only reacts to the switch.
    fn sync_target(&mut self, target: GraphRef) -> bool {
        if self.scene_target == Some(target) {
            return false;
        }
        self.main_window.reset_transient();
        self.scene_target = Some(target);
        true
    }

    /// Rebuild the `Scene` projection from the graph + view the `target`
    /// points at. Centralizes the borrow split (mut `scene`, shared
    /// `document` + `func_lib`) so the frame can rebuild at more than one
    /// point without repeating it. For a `Local` target, also hands the
    /// scene the enclosing `SubgraphDef` so the interior's boundary nodes
    /// can mirror its interface as their ports.
    ///
    /// Reconciles every subgraph's interface against its interior wiring
    /// first (derived state, like the scene itself) so boundary nodes
    /// render the right ports + placeholder and the doc is consistent
    /// before any save.
    fn rebuild_scene(&mut self, target: GraphRef) {
        self.document.reconcile_boundaries(&self.func_lib);
        let graph = self
            .document
            .graph_for(target)
            .expect("active tab graph exists");
        let view = self.document.view(target).expect("active tab view exists");
        let ctx_def = match target {
            GraphRef::Main => None,
            GraphRef::Local(id) => self.document.graph.subgraphs.by_key(&id),
        };
        self.scene.rebuild(graph, view, &self.func_lib, ctx_def);
    }

    /// Ctrl+Z / Ctrl+Shift+Z. Replays undo/redo against the document
    /// (each entry carries its own graph target). Returns whether a
    /// relayout is needed.
    ///
    /// The chords are sampled via `key_pressed` *every frame,
    /// unconditionally* — that call both reads the press and keeps the
    /// chord subscribed, and palantir's keyboard wake-gate only delivers
    /// an off-focus press when its chord was subscribed last frame
    /// (subscriptions clear each frame). Focus only gates the *action*:
    /// while a widget holds focus, Ctrl+Z must undo that widget's text,
    /// so the graph-level handling stands down.
    fn apply_undo_redo(&mut self, ui: &mut Ui) -> bool {
        let undo = ui.key_pressed(UNDO_SHORTCUT);
        let redo = ui.key_pressed(REDO_SHORTCUT);
        if ui.focused_id().is_some() {
            return false;
        }
        let mut relayout = false;
        let mut on_step = |step: &intent::UndoStep| {
            relayout |= requires_relayout(step);
        };
        if undo {
            self.action_stack.undo(&mut self.document, &mut on_step);
        } else if redo {
            self.action_stack.redo(&mut self.document, &mut on_step);
        }
        relayout
    }

    /// Esc-deselect and Ctrl+0 reset-zoom — both act on the active view,
    /// so they take the settled `target`. Routed through the intent stack
    /// (not a direct doc write) so they land in the undo history; the
    /// `is_noop` filter in `drain_intents` drops them when they'd change
    /// nothing. Chords are sampled unconditionally (see `apply_undo_redo`)
    /// and gated by focus.
    fn apply_canvas_shortcuts(&mut self, ui: &mut Ui, target: GraphRef) -> bool {
        let reset_zoom = ui.key_pressed(RESET_ZOOM_SHORTCUT);
        let escape = ui.escape_pressed();
        if ui.focused_id().is_some() {
            return false;
        }
        let view = self.document.view(target).expect("active tab view exists");
        let has_selection = !view.selected_nodes.is_empty();
        let pan = view.pan;
        if escape && has_selection {
            self.intents.push(Intent::SetSelection {
                to: BTreeSet::new(),
            });
        }
        if reset_zoom {
            self.intents.push(Intent::SetViewport { pan, scale: 1.0 });
        }
        false
    }

    /// Map Ctrl+N / Ctrl+O / Ctrl+S / Ctrl+Shift+S to a `MenuCommand`.
    ///
    /// Document file ops are **global** — they fire regardless of
    /// focus, so Ctrl+S still saves while a node's value editor is
    /// focused (TextEdit doesn't bind S/O/N, so nothing is stolen).
    /// Every chord is sampled with `key_pressed` each frame so all
    /// stay subscribed for palantir's wake-gate (sampling all four up
    /// front, not short-circuited, so one chord firing doesn't drop
    /// the others' subscription that frame). Save-As (Ctrl+Shift+S) is
    /// checked before Save (Ctrl+S) so the shift variant wins its
    /// combo. Theme actions are menu-only — no shortcut.
    fn menu_shortcut(&self, ui: &mut Ui) -> Option<MenuCommand> {
        let new = ui.key_pressed(NEW_SHORTCUT);
        let open = ui.key_pressed(OPEN_SHORTCUT);
        let save_as = ui.key_pressed(SAVE_AS_SHORTCUT);
        let save = ui.key_pressed(SAVE_SHORTCUT);
        if new {
            Some(MenuCommand::NewDocument)
        } else if open {
            Some(MenuCommand::LoadDocument)
        } else if save_as {
            Some(MenuCommand::SaveDocumentAs)
        } else if save {
            Some(MenuCommand::SaveDocument)
        } else {
            None
        }
    }

    fn handle_menu_command(&mut self, ui: &mut Ui, command: MenuCommand) {
        match command {
            MenuCommand::NewDocument => self.new_document(),
            MenuCommand::LoadDocument => {
                if let Some(path) = persistence::pick_open_path(self.current_path.as_deref()) {
                    self.load_document(&path);
                }
            }
            MenuCommand::SaveDocument => self.save_current(),
            MenuCommand::SaveDocumentAs => self.save_document_as(),
            MenuCommand::LoadTheme => {
                if let Some(path) = persistence::pick_theme_open() {
                    self.load_theme(ui, &path);
                }
            }
            MenuCommand::ExportTheme => {
                if let Some(path) = persistence::pick_theme_save() {
                    persistence::export_theme(&self.theme, &path);
                }
            }
            MenuCommand::ExportSubgraph => self.export_active_subgraph(),
            MenuCommand::ImportSubgraph => self.import_subgraph(),
        }
    }

    /// Export a subgraph def to a file (its interior `Graph` carries any
    /// nested subgraph defs along). A selected subgraph-instance node
    /// wins; otherwise, when the active tab is itself a subgraph, that
    /// open subgraph is exported. No-op when neither resolves.
    fn export_active_subgraph(&mut self) {
        let Some(def) = self.subgraph_to_export() else {
            eprintln!("subgraph export: no subgraph selected or open");
            return;
        };
        if let Some(path) = persistence::pick_subgraph_save(self.current_path.as_deref()) {
            persistence::export_subgraph(def, &path);
        }
    }

    /// Resolve which subgraph def an export targets: the first selected
    /// subgraph-instance node in the active graph (resolved against that
    /// graph's own `Local` table or the shared `FuncLib` for `Linked`),
    /// else the currently open subgraph when inside one.
    fn subgraph_to_export(&self) -> Option<&SubgraphDef> {
        let target = self.document.active_target();
        let graph = self.document.graph_for(target)?;
        if let Some(view) = self.document.view(target) {
            for nid in &view.selected_nodes {
                if let Some(node) = graph.by_id(nid)
                    && let NodeKind::Subgraph(r) = node.kind
                    && let Some(def) = graph.resolve_def(r, &self.func_lib)
                {
                    return Some(def);
                }
            }
        }
        match target {
            GraphRef::Local(id) => self.document.graph.subgraphs.by_key(&id),
            GraphRef::Main => None,
        }
    }

    /// Import a subgraph def from a file as a local def in the current
    /// document. The import is a copy with a fresh id; nothing is
    /// instantiated and the undo stack is untouched (existing history
    /// references no imported def, so it stays valid).
    fn import_subgraph(&mut self) {
        let Some(path) = persistence::pick_subgraph_open(self.current_path.as_deref()) else {
            return;
        };
        if let Some(def) = persistence::import_subgraph(&path) {
            self.document.import_subgraph(def);
        }
    }

    /// Replace the document with an empty one and reset all derived
    /// state. Clears the undo stack — restoring the previous doc via
    /// Cmd-Z would re-introduce all of its nodes one-step-at-a-time
    /// from intent history that no longer matches the live tree.
    fn new_document(&mut self) {
        self.document = Document::default();
        self.action_stack.clear();
        self.intents.clear();
        // Force a scene rebuild next frame: the active target may still
        // be `Main`, but it now points at a different graph.
        self.scene_target = None;
        self.set_document_path(None);
    }

    fn load_document(&mut self, path: &Path) {
        let Some(doc) = persistence::load_document(path) else {
            return;
        };
        self.document = doc;
        self.action_stack.clear();
        self.intents.clear();
        self.scene_target = None;
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
        if let Some(path) = persistence::pick_save_path(self.current_path.as_deref()) {
            self.save_document(&path);
        }
    }

    fn save_document(&mut self, path: &Path) {
        if persistence::save_document(&self.document, path) {
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

    /// Load a theme picked from the dialog: copy it into the working
    /// dir under its own name (so the config can reference it by name
    /// across sessions), apply it, and persist the name. The picked
    /// file may live anywhere; the working-dir copy is the canonical
    /// one the config resolves on the next launch.
    fn load_theme(&mut self, ui: &mut Ui, picked: &Path) {
        let Some(stem) = picked.file_stem().and_then(|s| s.to_str()) else {
            eprintln!("theme load failed: path has no file name");
            return;
        };
        let dest = AppConfig::theme_path(stem);
        // Only copy when the picked file isn't already the working-dir
        // copy (re-loading the active theme shouldn't self-overwrite).
        if picked != dest
            && let Err(err) = std::fs::copy(picked, &dest)
        {
            eprintln!("theme load failed: copy to {}: {err}", dest.display());
            return;
        }
        if self.load_theme_file(&dest) {
            self.config.theme_name = Some(stem.to_owned());
            self.config.save();
            ui.theme = self.theme.palantir_theme.clone();
        }
    }

    /// Apply a theme `.toml` from `path` into `self.theme`. Returns
    /// whether it succeeded; on failure leaves the current theme
    /// untouched. Shared by startup restore and menu load.
    fn load_theme_file(&mut self, path: &Path) -> bool {
        match persistence::load_theme(path) {
            Some(theme) => {
                self.theme = theme;
                true
            }
            None => false,
        }
    }

    /// Drain `intents`, applying each non-no-op intent to `document`,
    /// and push the whole frame's resulting steps onto the undo stack
    /// as a single batch entry — so a gesture that emits N intents
    /// (e.g. breaker swipe deleting K nodes + unbinding M ports) is
    /// one Cmd-Z. Marks the scene dirty when anything applied (so the
    /// pre-record rebuild folds the change in). Returns whether any
    /// applied step needs a relayout retry.
    fn drain_intents(&mut self, target: GraphRef) -> bool {
        let mut relayout = false;
        let mut batch = Vec::new();
        for intent in self.intents.drain(..) {
            let Some(step) = build_step(intent, &self.document, target) else {
                continue;
            };
            if step.is_noop() {
                continue;
            }
            apply_step(&step, &mut self.document, target);
            relayout |= requires_relayout(&step);
            batch.push(step);
        }
        if !batch.is_empty() {
            self.scene_dirty = true;
            self.action_stack.push_current(target, &batch);
        }
        relayout
    }

    /// Keep the tab list renderable: drop tabs whose graph vanished,
    /// seed any `Local` tab that's missing its view metadata, and clamp
    /// `active` into range — so `rebuild_scene` always resolves a live
    /// graph *and* view. `Main` always survives (`graph_for(Main)` is
    /// infallible and `main_view` always exists).
    ///
    /// The view-seeding covers a desync hazard: `tabs` and `sub_views`
    /// are independent serialized fields, so a deserialized (or
    /// hand-edited) document can carry a `Local` tab with no matching
    /// `sub_views` entry. Seeding it here recovers gracefully instead of
    /// panicking on the `view(target).expect(..)` in `rebuild_scene`.
    fn ensure_valid_active(&mut self) {
        // Common case: every tab still resolves — touch nothing (no
        // per-frame allocation). Only rebuild the list when a tab's
        // graph actually vanished.
        if self
            .document
            .tabs
            .iter()
            .any(|t| self.document.graph_for(*t).is_none())
        {
            // Split the borrow so `retain` (mut `tabs`) can read `graph`.
            let Document { graph, tabs, .. } = &mut self.document;
            tabs.retain(|t| match t {
                GraphRef::Main => true,
                GraphRef::Local(id) => graph.subgraphs.by_key(id).is_some(),
            });
        }
        // Seed views for any `Local` tab missing one. Guarded by `any`
        // so the common (all-seeded) case allocates nothing.
        if self
            .document
            .tabs
            .iter()
            .any(|t| matches!(t, GraphRef::Local(id) if !self.document.sub_views.contains_key(id)))
        {
            let missing: Vec<SubgraphId> = self
                .document
                .tabs
                .iter()
                .filter_map(|t| match t {
                    GraphRef::Local(id) if !self.document.sub_views.contains_key(id) => Some(*id),
                    _ => None,
                })
                .collect();
            for id in missing {
                self.ensure_sub_view(id);
            }
        }
        if self.document.active >= self.document.tabs.len() {
            self.document.active = self.document.tabs.len() - 1;
        }
    }

    /// Build the strip's per-tab labels from the open-tab list.
    fn tab_labels(&self) -> Vec<TabLabel> {
        self.document
            .tabs
            .iter()
            .map(|t| match t {
                GraphRef::Main => TabLabel {
                    text: "main".into(),
                    closable: false,
                },
                GraphRef::Local(id) => {
                    let name = self
                        .document
                        .graph
                        .subgraphs
                        .by_key(id)
                        .map(|d| d.name.clone())
                        .unwrap_or_else(|| "subgraph".to_string());
                    TabLabel {
                        text: name.into(),
                        closable: true,
                    }
                }
            })
            .collect()
    }

    /// Apply the record pass's view-state requests. Open mutates the tab
    /// list directly (not undoable); activate and close are queued as
    /// `Intent::SwitchTab` / `Intent::CloseTab` so they join the undo
    /// history. Returns whether a relayout is needed (the queued intents'
    /// relayout is decided later, when they drain).
    fn apply_view_actions(&mut self) -> bool {
        let mut relayout = false;
        for action in std::mem::take(&mut self.actions) {
            match action {
                UiAction::OpenGraph(target) => relayout |= self.open_graph(target),
                UiAction::ActivateTab(index) => {
                    self.intents.push(Intent::SwitchTab { to: index });
                }
                UiAction::CloseTab(index) => {
                    self.intents.push(Intent::CloseTab { index });
                }
            }
        }
        relayout
    }

    /// Focus `target`'s tab, opening a new one (lazily seeding its view
    /// metadata) if not already open. Returns whether anything changed.
    fn open_graph(&mut self, target: GraphRef) -> bool {
        if let Some(index) = self.document.tabs.iter().position(|t| *t == target) {
            if self.document.active != index {
                self.document.active = index;
                return true;
            }
            return false;
        }
        if let GraphRef::Local(id) = target
            && !self.ensure_sub_view(id)
        {
            return false;
        }
        self.document.tabs.push(target);
        self.document.active = self.document.tabs.len() - 1;
        true
    }

    /// Ensure a `GraphView` exists for a local subgraph interior,
    /// auto-laying-out its nodes on first creation. Returns `false` if
    /// the subgraph no longer exists.
    fn ensure_sub_view(&mut self, id: SubgraphId) -> bool {
        if self.document.sub_views.contains_key(&id) {
            return true;
        }
        let view = {
            let Some(def) = self.document.graph.subgraphs.by_key(&id) else {
                return false;
            };
            let mut view = GraphView::for_graph(&def.graph);
            view.auto_layout(&def.graph, 220.0, 110.0, Vec2::new(40.0, 40.0));
            view
        };
        self.document.sub_views.insert(id, view);
        true
    }
}
