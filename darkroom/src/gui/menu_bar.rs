use std::sync::Arc;

use glam::Vec2;
use palantir::{Button, Configure, ContextMenu, MenuItem, Panel, PopupHandle, Sizing, Spacing, Ui};
use scenarium::data::FsPathConfig;
use scenarium::prelude::NodeId;

use crate::core::theme_pref::ThemeChoice;
use crate::gui::HostHandle;

/// A command surfaced by the menu bar. `App` performs the side effect
/// (file dialog + read/write + doc/theme swap + config persist)
/// outside the record pass — keeps `menu_bar` decoupled from
/// `Document` / `Theme` / `ActionStack` and lets the blocking dialog
/// run without holding borrows from the active frame.
#[derive(Clone, Debug)]
pub(crate) enum MenuCommand {
    NewDocument,
    LoadDocument,
    /// Save to the current file, or prompt (Save As) if there isn't one.
    SaveDocument,
    /// Always prompt for a destination.
    SaveDocumentAs,
    LoadTheme,
    ExportTheme,
    /// Set the theme preference: `System` follows the OS light/dark
    /// setting, `Dark`/`Light` pin a palette. Persisted to config.
    SetTheme(ThemeChoice),
    /// Export the active subgraph (plus its local-def dependencies) to a
    /// file. No-op when the active tab isn't a subgraph.
    ExportSubgraph,
    /// Import a subgraph bundle from a file into the current document.
    ImportSubgraph,
    /// Publish a copy of the active subgraph into the shared library
    /// (`Library`), so it can be instanced as `Linked` anywhere. No-op
    /// when the active tab / selection isn't a subgraph.
    PromoteSubgraph,
    /// Publish a specific node's local subgraph def to the library (the
    /// S-badge "Publish" action). Updates the library def it came from
    /// in place when linked (`origin`), else creates a new entry and
    /// links the local def to it.
    PublishNodeSubgraph {
        node_id: NodeId,
    },
    /// Open a file dialog (filtered by `config`) for a node's `FsPath`
    /// const input, applying the chosen path as a `SetInput` edit. Raised
    /// by the inline pick button (see `gui::node::emit_path_picks`); the
    /// blocking dialog runs outside the record like the other file ops.
    PickInputPath {
        node_id: NodeId,
        port_idx: usize,
        config: Arc<FsPathConfig>,
    },
    /// Evaluate the graph once on the worker.
    Run,
    /// Request cancellation of the in-flight run.
    CancelRun,
    /// Open (or focus) the Config tab — the app-settings window.
    OpenConfig,
    /// Open an ONNX file dialog for one of the ML model paths and persist
    /// the choice. Raised by the Config tab's "Browse…" buttons.
    PickMlModel(MlModelKind),
}

/// Which ML model path a [`MenuCommand::PickMlModel`] targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MlModelKind {
    /// The `ml_denoise` node's model (DeepSNR).
    Denoise,
    /// The `remove_stars` node's model (StarNet).
    StarRemoval,
}

/// Top-of-window menu bar. Horizontal strip of "menu trigger" buttons;
/// each opens a [`ContextMenu`] anchored at the trigger's bottom-left.
/// `Quit` calls into [`HostHandle::quit`]; everything else returns a
/// [`MenuCommand`] for `App` to consume.
pub(crate) fn show(ui: &mut Ui, host: Option<&HostHandle>, running: bool) -> Option<MenuCommand> {
    let mut command = None;
    Panel::hstack()
        .auto_id()
        .size((Sizing::FILL, Sizing::Hug))
        .padding(Spacing::xy(4.0, 4.0))
        .gap(2.0)
        .show(ui, |ui| {
            command = file_menu(ui, host)
                .or_else(|| view_menu(ui))
                .or_else(|| subgraph_menu(ui))
                .or_else(|| run_menu(ui, running));
        });
    command
}

/// One menu-bar dropdown: a flat trigger button that toggles a
/// `ContextMenu` of [`MenuItem`] rows. `build` populates the popup and
/// returns the chosen command, if any. Centralizes the trigger +
/// anchor + open plumbing so each menu is just its label + rows.
fn dropdown(
    ui: &mut Ui,
    label: &'static str,
    build: impl FnOnce(&mut Ui, &PopupHandle) -> Option<MenuCommand>,
) -> Option<MenuCommand> {
    let trigger = Button::new()
        .label(label)
        .style(ui.theme.menu_button.clone())
        .show(ui)
        .snapshot();
    if trigger.clicked()
        && let Some(rect) = trigger.rect()
    {
        ContextMenu::open(ui, trigger.widget_id(), Vec2::new(rect.min.x, rect.max().y));
    }
    let mut command = None;
    ContextMenu::for_id(trigger.widget_id()).show(ui, |ui, popup| {
        command = build(ui, popup);
    });
    command
}

fn file_menu(ui: &mut Ui, host: Option<&HostHandle>) -> Option<MenuCommand> {
    dropdown(ui, "File", |ui, popup| {
        let mut command = None;
        if MenuItem::new("New").show(ui, popup).clicked() {
            command = Some(MenuCommand::NewDocument);
        }
        if MenuItem::new("Load…").show(ui, popup).clicked() {
            command = Some(MenuCommand::LoadDocument);
        }
        if MenuItem::new("Save").show(ui, popup).clicked() {
            command = Some(MenuCommand::SaveDocument);
        }
        if MenuItem::new("Save As…").show(ui, popup).clicked() {
            command = Some(MenuCommand::SaveDocumentAs);
        }
        MenuItem::separator(ui);
        if MenuItem::new("Quit").show(ui, popup).clicked()
            && let Some(h) = host
        {
            h.quit();
        }
        command
    })
}

fn view_menu(ui: &mut Ui) -> Option<MenuCommand> {
    dropdown(ui, "View", |ui, popup| {
        let mut command = None;
        if MenuItem::new("Config").show(ui, popup).clicked() {
            command = Some(MenuCommand::OpenConfig);
        }
        command
    })
}

fn subgraph_menu(ui: &mut Ui) -> Option<MenuCommand> {
    dropdown(ui, "Subgraph", |ui, popup| {
        let mut command = None;
        if MenuItem::new("Export…").show(ui, popup).clicked() {
            command = Some(MenuCommand::ExportSubgraph);
        }
        if MenuItem::new("Import…").show(ui, popup).clicked() {
            command = Some(MenuCommand::ImportSubgraph);
        }
        if MenuItem::new("Promote to Library…")
            .show(ui, popup)
            .clicked()
        {
            command = Some(MenuCommand::PromoteSubgraph);
        }
        command
    })
}

fn run_menu(ui: &mut Ui, running: bool) -> Option<MenuCommand> {
    dropdown(ui, "Run", |ui, popup| {
        let mut command = None;
        if MenuItem::new("Run Once").show(ui, popup).clicked() {
            command = Some(MenuCommand::Run);
        }
        // Only offer Cancel while a run is actually in flight.
        if running && MenuItem::new("Cancel Run").show(ui, popup).clicked() {
            command = Some(MenuCommand::CancelRun);
        }
        command
    })
}
