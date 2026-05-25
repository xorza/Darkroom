use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use glam::Vec2;
use lens::ImageFuncLib;
use palantir::{HostHandle, Shortcut, Ui};
use scenarium::elements::basic_funclib::BasicFuncLib;
use scenarium::elements::worker_events_funclib::WorkerEventsFuncLib;
use scenarium::prelude::{FuncLib, SubgraphId};
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

const UNDO_HISTORY: usize = 100;

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
            main_window: MainWindow::default(),
            intents: Vec::new(),
            actions: Vec::new(),
            action_stack: ActionStack::new(UNDO_HISTORY),
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

        // A closed/deleted target can't be active; fall back to Main.
        self.ensure_valid_active();
        let target = self.document.active_target();
        let prev_active = self.document.active;

        // Prepass: each UI subtree pushes input-derived intents
        // (drag-driven `MoveNode`, etc.) into `intents`. Drained and
        // applied *before* `Scene::rebuild`, so Pass A's record sees
        // the freshly-mutated doc — no Pass B retry for drag.
        self.intents.clear();
        self.actions.clear();
        // Prepass only derives intents from input state; it never draws
        // and reads everything it needs off `Scene`, so it takes neither
        // the func lib nor the full `AppContext`.
        self.main_window.prepass(ui, &self.scene, &mut self.intents);
        let mut relayout = self.drain_intents(target);
        relayout |= self.handle_shortcuts(ui, target);

        let command_from_shortcut = self.menu_shortcut(ui);

        // Record. Widgets push intents derived from record-time state
        // (button clicks, edit commits) into `intents`.
        let graph = self
            .document
            .graph_for(target)
            .expect("active tab graph exists");
        let view = self.document.view(target).expect("active tab view exists");
        self.scene.rebuild(graph, view, &self.func_lib);
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
                &mut self.actions,
            )
            .or(command_from_shortcut);

        // View actions (open/close a tab) mutate the tab list directly;
        // tab *switches* become an `Intent::SwitchTab` so they're
        // undoable and coalescing — both land before the post-record
        // drain below.
        relayout |= self.apply_view_actions();
        // Post-record drain — these intents reflect mutations that
        // only the now-just-completed record could surface (incl. the
        // switch intent just queued), so they warrant a relayout retry
        // when applicable.
        relayout |= self.drain_intents(target);
        // A changed active tab (via switch intent or open/close) swaps
        // the rendered graph; drop stale drag/connection bookkeeping.
        if self.document.active != prev_active {
            self.main_window.reset_transient();
            relayout = true;
        }
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
    /// Ctrl+Z / Ctrl+Shift+Z (undo/redo) and Esc-to-deselect.
    ///
    /// The chords are sampled via `key_pressed` *every frame,
    /// unconditionally* — that call both reads the press and keeps the
    /// chord subscribed, and palantir's keyboard wake-gate only
    /// delivers an off-focus press when its chord was subscribed last
    /// frame (subscriptions are cleared each frame). Early-returning
    /// before `key_pressed` would let the subscription lapse and park
    /// presses. Focus only gates the *action*: while a widget holds
    /// focus, Ctrl+Z must undo that widget's text and Esc must blur it,
    /// so the graph-level handling stands down.
    fn handle_shortcuts(&mut self, ui: &mut Ui, target: GraphRef) -> bool {
        let undo = ui.key_pressed(UNDO_SHORTCUT);
        let redo = ui.key_pressed(REDO_SHORTCUT);
        let reset_zoom = ui.key_pressed(RESET_ZOOM_SHORTCUT);
        let escape = ui.escape_pressed();
        if ui.focused_id().is_some() {
            return false;
        }
        let mut relayout = false;
        {
            let mut on_step = |step: &intent::UndoStep| {
                relayout |= requires_relayout(step);
            };
            if undo {
                self.action_stack.undo(&mut self.document, &mut on_step);
            } else if redo {
                self.action_stack.redo(&mut self.document, &mut on_step);
            }
        }
        // Esc clears the selection. Routed through the intent stack
        // (not a direct doc write) so it lands in the undo history and
        // the batched relayout-detection path catches it like any other
        // selection change.
        let view = self.document.view(target).expect("active tab view exists");
        let has_selection = !view.selected_nodes.is_empty();
        let pan = view.pan;
        if escape && has_selection {
            self.intents.push(Intent::SetSelection {
                to: BTreeSet::new(),
            });
        }
        // Ctrl+0 resets zoom to 100% (keeping pan), via the same
        // `SetViewport` intent the pan/zoom gesture uses — so it's
        // undoable and persists. `is_noop` filters it when already 1.0.
        if reset_zoom {
            self.intents.push(Intent::SetViewport { pan, scale: 1.0 });
        }
        relayout
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
        self.set_document_path(None);
    }

    fn load_document(&mut self, path: &Path) {
        let Some(doc) = persistence::load_document(path) else {
            return;
        };
        self.document = doc;
        self.action_stack.clear();
        self.intents.clear();
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
    /// one Cmd-Z. Returns whether any applied step needs a relayout
    /// retry.
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
            self.action_stack.push_current(target, &batch);
        }
        relayout
    }

    /// Drop tabs whose graph vanished and clamp `active` into range, so
    /// the frame always has a live target to render. `Main` always
    /// survives (`graph_for(Main)` is infallible).
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

    /// Apply the record pass's view-state requests. Open/close mutate
    /// the tab list directly (not undoable); activate is queued as an
    /// `Intent::SwitchTab` so it joins the undo history. Returns whether
    /// a relayout is needed.
    fn apply_view_actions(&mut self) -> bool {
        let mut relayout = false;
        for action in std::mem::take(&mut self.actions) {
            match action {
                UiAction::OpenGraph(target) => relayout |= self.open_graph(target),
                UiAction::ActivateTab(index) => {
                    self.intents.push(Intent::SwitchTab { to: index });
                }
                UiAction::CloseTab(index) => relayout |= self.close_tab(index),
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

    /// Close the tab at `index` (the `Main` tab at 0 is never closable).
    /// Keeps the subgraph's view metadata so reopening restores its
    /// layout. Returns whether anything changed.
    fn close_tab(&mut self, index: usize) -> bool {
        if index == 0 || index >= self.document.tabs.len() {
            return false;
        }
        self.document.tabs.remove(index);
        if self.document.active > index {
            self.document.active -= 1;
        }
        self.document.active = self.document.active.min(self.document.tabs.len() - 1);
        true
    }
}
