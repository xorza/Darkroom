use std::path::{Path, PathBuf};

use common::{SerdeFormat, deserialize, serialize};
use glam::Vec2;
use lens::ImageFuncLib;
use palantir::{HostHandle, Shortcut, Ui};
use scenarium::data::StaticValue;
use scenarium::elements::basic_funclib::BasicFuncLib;
use scenarium::elements::worker_events_funclib::WorkerEventsFuncLib;
use scenarium::graph::Binding;
use scenarium::prelude::{FuncLib, NodeId};
use scenarium::testing::{TestFuncHooks, test_func_lib, test_graph};

use crate::action_stack::ActionStack;
use crate::config::AppConfig;
use crate::document::Document;
use crate::gui::main_window::MainWindow;
use crate::gui::menu_bar::MenuCommand;
use crate::intent::{self, Intent, apply_step, build_step, requires_relayout};
use crate::scene::Scene;
use crate::theme::Theme;

const UNDO_SHORTCUT: Shortcut = Shortcut::cmd('Z');
const REDO_SHORTCUT: Shortcut = Shortcut::cmd_shift('Z');
const NEW_SHORTCUT: Shortcut = Shortcut::cmd('N');
const OPEN_SHORTCUT: Shortcut = Shortcut::cmd('O');
const SAVE_SHORTCUT: Shortcut = Shortcut::cmd('S');

/// File-dialog extension filters. First entry is the default — Rhai
/// is the canonical on-disk format for scenarium graphs (matches the
/// deprecated-darkroom file menu's filter list).
const FILE_FILTERS: &[(&str, &[&str])] = &[
    ("Rhai", &["rhai"]),
    ("JSON", &["json"]),
    ("Lz4 compressed Rhai", &["lz4"]),
    ("TOML", &["toml"]),
];

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

impl<'a> AppContext<'a> {
    pub fn new(theme: &'a Theme, func_lib: &'a FuncLib) -> Self {
        Self { theme, func_lib }
    }
}

#[derive(Debug)]
pub struct App {
    pub document: Document,
    pub func_lib: FuncLib,
    pub scene: Scene,
    pub main_window: MainWindow,
    pub intents: Vec<Intent>,
    pub action_stack: ActionStack,
    pub theme: Theme,
    pub host_handle: Option<HostHandle>,
    /// Last successfully loaded/saved file path. `Save…` and `Load…`
    /// preopen the dialog at this directory so a session that touches
    /// many files in the same folder doesn't re-navigate each time.
    pub current_path: Option<PathBuf>,
    /// Persisted session state (active theme name + last document).
    /// Written on every doc/theme change so the next launch reopens
    /// where the user left off.
    pub config: AppConfig,
}

impl App {
    pub fn new() -> Self {
        let mut document: Document = test_graph().into();
        seed_const_bindings(&mut document);
        document.auto_layout(220.0, 110.0, Vec2::new(40.0, 40.0));
        let mut func_lib = FuncLib::default();
        func_lib.merge(test_func_lib(TestFuncHooks::default()));
        func_lib.merge(BasicFuncLib::default());
        func_lib.merge(WorkerEventsFuncLib::default());
        func_lib.merge(ImageFuncLib::default());
        Self {
            document,
            func_lib,
            scene: Scene::default(),
            main_window: MainWindow::default(),
            intents: Vec::new(),
            action_stack: ActionStack::new(UNDO_HISTORY),
            theme: Theme::default(),
            host_handle: None,
            current_path: None,
            config: AppConfig::default(),
        }
    }

    /// One-shot startup wiring, run from `WinitHost::with_setup`
    /// before the first frame. Loads the persisted config, restores
    /// the saved theme + document, and pushes the resolved palantir
    /// theme onto `Ui`. Failures degrade silently to defaults — a
    /// missing/corrupt config or a deleted document must not block
    /// launch.
    pub fn startup(&mut self, ui: &mut Ui) {
        self.config = AppConfig::load();
        if let Some(name) = self.config.theme_name.clone() {
            self.load_theme_file(&AppConfig::theme_path(&name));
        }
        if let Some(path) = self.config.document_path.clone() {
            self.load_document(&path);
        }
        // Resolved theme (default, or whatever the config restored)
        // onto the Ui so palantir widgets paint correctly frame 1.
        ui.theme = self.theme.palantir_theme.clone();
    }
}

impl palantir::App for App {
    fn frame(&mut self, ui: &mut Ui) {
        // ui.debug_overlay.damage_rect = true;

        // Prepass: each UI subtree pushes input-derived intents
        // (drag-driven `MoveNode`, etc.) into `intents`. Drained and
        // applied *before* `Scene::rebuild`, so Pass A's record sees
        // the freshly-mutated doc — no Pass B retry for drag.
        self.intents.clear();
        self.main_window.prepass(ui, &self.scene, &mut self.intents);
        let mut relayout = self.drain_intents();
        relayout |= self.handle_shortcuts(ui);

        let command_from_shortcut = self.menu_shortcut(ui);

        // Record. Widgets push intents derived from record-time state
        // (button clicks, edit commits) into `intents`.
        self.scene.rebuild(&self.document, &self.func_lib);
        let ctx = AppContext::new(&self.theme, &self.func_lib);
        let host = self.host_handle.clone();
        let command = self
            .main_window
            .frame(ui, &ctx, &mut self.scene, host.as_ref(), &mut self.intents)
            .or(command_from_shortcut);

        // Post-record drain — these intents reflect mutations that
        // only the now-just-completed record could surface, so they
        // *do* warrant a relayout retry when applicable.
        if self.drain_intents() | relayout {
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

/// Build an `rfd::FileDialog` preconfigured with the project's
/// extension filters and (optionally) a starting directory taken from
/// the last opened/saved path's parent. Shared by the open and save
/// flows so both surfaces stay in sync.
fn file_dialog(start: Option<&Path>) -> rfd::FileDialog {
    let mut dialog = rfd::FileDialog::new();
    for (name, exts) in FILE_FILTERS {
        dialog = dialog.add_filter(*name, exts);
    }
    if let Some(parent) = start.and_then(Path::parent) {
        dialog = dialog.set_directory(parent);
    }
    dialog
}

fn pick_open_path(start: Option<&Path>) -> Option<PathBuf> {
    file_dialog(start).pick_file()
}

fn pick_save_path(start: Option<&Path>) -> Option<PathBuf> {
    file_dialog(start).save_file()
}

/// TOML-only dialog for theme files. Themes always round-trip through
/// TOML (the format the config references by name), so there's no
/// multi-format picker like documents have.
fn theme_dialog() -> rfd::FileDialog {
    rfd::FileDialog::new().add_filter("TOML theme", &["toml"])
}

/// Replace a few of `test_graph`'s `Binding::Bind` inputs with
/// `Binding::Const(..)` so the inline static-value editor has
/// something to render on first launch. Targets the `mult` and `sum`
/// nodes by their well-known ids from `scenarium::testing::test_graph`.
fn seed_const_bindings(doc: &mut Document) {
    let mult_id: NodeId = "579ae1d6-10a3-4906-8948-135cb7d7508b".into();
    let sum_id: NodeId = "999c4d37-e0eb-4856-be3f-ad2090c84d8c".into();
    if let Some(node) = doc.graph.by_id_mut(&mult_id) {
        node.inputs[1].binding = Binding::Const(StaticValue::Int(7));
    }
    if let Some(node) = doc.graph.by_id_mut(&sum_id) {
        node.inputs[1].binding = Binding::Const(StaticValue::Float(2.5));
    }
}

impl App {
    /// Handle Cmd+Z / Cmd+Shift+Z and Esc-to-deselect. Suppressed
    /// while a widget holds keyboard focus so Cmd+Z inside a TextEdit
    /// doesn't nuke the graph and Esc inside a TextEdit blurs the
    /// editor instead of clearing selection.
    fn handle_shortcuts(&mut self, ui: &mut Ui) -> bool {
        if ui.focused_id().is_some() {
            return false;
        }
        let mut relayout = false;
        let mut on_step = |step: &intent::UndoStep| {
            relayout |= requires_relayout(step);
        };
        if ui.key_pressed(UNDO_SHORTCUT) {
            self.action_stack.undo(&mut self.document, &mut on_step);
        } else if ui.key_pressed(REDO_SHORTCUT) {
            self.action_stack.redo(&mut self.document, &mut on_step);
        }
        // Esc deselects. Routed through the intent stack (not a
        // direct doc write) so it lands in the undo history and the
        // batched relayout-detection path catches it like any other
        // selection change.
        if ui.escape_pressed() && self.document.selected_node_id.is_some() {
            self.intents.push(Intent::SelectNode { to: None });
        }
        relayout
    }

    /// Map Cmd+N / Cmd+O / Cmd+S to a `MenuCommand`. Gated on no
    /// keyboard focus so Cmd+S inside a TextEdit doesn't escape into
    /// a save dialog. Cmd+N wins over open/save if multiple fire on
    /// the same frame (no realistic combo, but the priority is
    /// stable). Theme actions are menu-only — no shortcut.
    fn menu_shortcut(&self, ui: &mut Ui) -> Option<MenuCommand> {
        if ui.focused_id().is_some() {
            return None;
        }
        if ui.key_pressed(NEW_SHORTCUT) {
            Some(MenuCommand::NewDocument)
        } else if ui.key_pressed(OPEN_SHORTCUT) {
            Some(MenuCommand::LoadDocument)
        } else if ui.key_pressed(SAVE_SHORTCUT) {
            Some(MenuCommand::SaveDocument)
        } else {
            None
        }
    }

    fn handle_menu_command(&mut self, ui: &mut Ui, command: MenuCommand) {
        match command {
            MenuCommand::NewDocument => self.new_document(),
            MenuCommand::LoadDocument => {
                if let Some(path) = pick_open_path(self.current_path.as_deref()) {
                    self.load_document(&path);
                }
            }
            MenuCommand::SaveDocument => {
                if let Some(path) = pick_save_path(self.current_path.as_deref()) {
                    self.save_document(&path);
                }
            }
            MenuCommand::LoadTheme => {
                if let Some(path) = theme_dialog().pick_file() {
                    self.load_theme(ui, &path);
                }
            }
            MenuCommand::ExportTheme => {
                if let Some(path) = theme_dialog().save_file() {
                    self.export_theme(&path);
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
        let format = match SerdeFormat::from_file_name(&path.to_string_lossy()) {
            Ok(f) => f,
            Err(err) => {
                eprintln!("load failed: unsupported file extension ({err})");
                return;
            }
        };
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(err) => {
                eprintln!("load failed: {} {err}", path.display());
                return;
            }
        };
        match Document::deserialize(format, &bytes) {
            Ok(doc) => {
                self.document = doc;
                self.action_stack.clear();
                self.intents.clear();
                self.set_document_path(Some(path.to_path_buf()));
            }
            Err(err) => eprintln!("load failed: {} {err}", path.display()),
        }
    }

    fn save_document(&mut self, path: &Path) {
        let format = match SerdeFormat::from_file_name(&path.to_string_lossy()) {
            Ok(f) => f,
            Err(_) => SerdeFormat::Rhai,
        };
        let bytes = self.document.serialize(format);
        match std::fs::write(path, &bytes) {
            Ok(()) => self.set_document_path(Some(path.to_path_buf())),
            Err(err) => eprintln!("save failed: {} {err}", path.display()),
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

    /// Read + deserialize a theme `.rhai` into `self.theme`. Returns
    /// whether it succeeded; on failure leaves the current theme
    /// untouched and logs. Shared by startup restore and menu load.
    fn load_theme_file(&mut self, path: &Path) -> bool {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(err) => {
                eprintln!("theme load failed: {} {err}", path.display());
                return false;
            }
        };
        match deserialize::<Theme>(&bytes, SerdeFormat::Toml) {
            Ok(theme) => {
                self.theme = theme;
                true
            }
            Err(err) => {
                eprintln!("theme load failed: {} {err}", path.display());
                false
            }
        }
    }

    fn export_theme(&self, path: &Path) {
        let bytes = serialize(&self.theme, SerdeFormat::Toml);
        if let Err(err) = std::fs::write(path, &bytes) {
            eprintln!("theme export failed: {} {err}", path.display());
        }
    }

    /// Drain `intents`, applying each non-no-op intent to `document`,
    /// and push the whole frame's resulting steps onto the undo stack
    /// as a single batch entry — so a gesture that emits N intents
    /// (e.g. breaker swipe deleting K nodes + unbinding M ports) is
    /// one Cmd-Z. Returns whether any applied step needs a relayout
    /// retry.
    fn drain_intents(&mut self) -> bool {
        let mut relayout = false;
        let mut batch = Vec::new();
        for intent in self.intents.drain(..) {
            let Some(step) = build_step(intent, &self.document) else {
                continue;
            };
            if step.is_noop() {
                continue;
            }
            apply_step(&step, &mut self.document);
            relayout |= requires_relayout(&step);
            batch.push(step);
        }
        if !batch.is_empty() {
            self.action_stack.push_current(&batch);
        }
        relayout
    }
}
