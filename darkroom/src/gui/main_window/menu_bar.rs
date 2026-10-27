use palantir::prelude::*;
use palantir::{CloseHandle, DockOperation, MenuSeparator};
use scenarium::NodeId;

use crate::core::document::tab_ref::TabRef;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::incoming_wires::IncomingWires;
use crate::gui::frame::app_command::{AppCommand, FileCommand, RunCommand, ViewCommand};
use crate::gui::frame::requests::Requests;
use crate::gui::frame::shortcuts;
use crate::gui::frame::window_ctx::WindowCtx;
use crate::gui::pane::graph_ui::gesture::pan_zoom::Framing;
use crate::gui::theme::Theme;
use crate::gui::widgets::support;

/// Top-of-window menu bar: the menu triggers at the left, each opening a [`ContextMenu`]
/// anchored at its bottom-left, and the document's name at the center, with a dot while it has
/// unsaved edits.
///
/// A pick goes onto `out` in whichever tier it belongs to: graph edits as intents, the file
/// lifecycle, the history, the run, the view and `Quit` as [`AppCommand`]s for `App` to run after
/// the pass, Preferences as the [`DockOperation`] that opens its tab. Every item with a chord
/// shows it, read from [`shortcuts`], the table its handler reads.
pub(crate) fn show(ui: &mut Ui, ctx: WindowCtx<'_>, out: &mut Requests) {
    let theme = ctx.app().theme();
    Panel::zstack()
        .id_salt("menu_bar")
        .size((Sizing::FILL, Sizing::HUG))
        .show(ui, |ui| {
            Panel::hstack()
                .id_salt("menu_bar_menus")
                .size((Sizing::HUG, Sizing::HUG))
                .align(Align::new(HAlign::Left, VAlign::Center))
                .padding(Spacing::xy(theme.space.sm, theme.space.xs))
                .gap(theme.space.xs)
                .show(ui, |ui| {
                    file_menu(ui, theme, out);
                    edit_menu(ui, ctx, out);
                    graph_menu(ui, ctx, out);
                    view_menu(ui, theme, out);
                });
            document_name(ui, ctx);
        });
}

/// One menu-bar dropdown: a flat trigger button labelled `label` that toggles
/// a `ContextMenu` of [`MenuItem`] rows. Centralizes the trigger + anchor +
/// open plumbing so each menu is just its label + rows.
#[derive(Clone, Copy, Debug)]
struct Dropdown {
    label: &'static str,
}

impl Dropdown {
    /// `build` populates the popup and raises whatever a pick means.
    fn show(self, ui: &mut Ui, theme: &Theme, build: impl FnOnce(&mut Ui, &CloseHandle)) {
        let id = WidgetId::from_hash(("menu_bar", self.label));
        let style = if ContextMenu::is_open(ui, id) {
            &theme.menu_button_open
        } else {
            &theme.menu_button
        };
        let trigger = Button::new()
            .id(id)
            .label(self.label)
            .style(style)
            .show(ui)
            .snapshot();
        if trigger.left.clicked()
            && let Some(rect) = trigger.rect
        {
            ContextMenu::open(ui, trigger.id, Vec2::new(rect.min.x, rect.max().y));
        }
        ContextMenu::for_id(trigger.id).show(ui, |ui, popup| {
            build(ui, popup);
        });
    }
}

/// One row, `enabled` or greyed out, with the chord its handler reads shown beside it; `true`
/// when picked.
fn item(
    ui: &mut Ui,
    popup: &CloseHandle,
    label: &'static str,
    shortcut: Option<Shortcut>,
    enabled: bool,
) -> bool {
    let row = MenuItem::new(label).disabled(!enabled);
    let row = match shortcut {
        Some(shortcut) => row.shortcut_hint(shortcut),
        None => row,
    };
    row.show(ui, popup).left.clicked()
}

fn file_menu(ui: &mut Ui, theme: &Theme, out: &mut Requests) {
    Dropdown { label: "File" }.show(ui, theme, |ui, popup| {
        let file = [
            ("New", Some(shortcuts::NEW), FileCommand::New),
            ("Open…", Some(shortcuts::OPEN), FileCommand::Open),
            ("Save", Some(shortcuts::SAVE), FileCommand::Save),
            ("Save As…", Some(shortcuts::SAVE_AS), FileCommand::SaveAs),
        ];
        for (label, shortcut, command) in file {
            if item(ui, popup, label, shortcut, true) {
                out.push_app(AppCommand::File(command));
            }
        }
        MenuSeparator::new().show(ui);
        if item(ui, popup, "Preferences", None, true) {
            out.push_view(DockOperation::OpenTab {
                tab: TabRef::Preferences,
            });
        }
        MenuSeparator::new().show(ui);
        if item(ui, popup, "Quit", Some(shortcuts::QUIT), true) {
            out.push_app(AppCommand::Quit);
        }
    });
}

/// The history and the selection's edits. Duplicate drops the wires from producers outside the
/// selection, as Ctrl+D does.
fn edit_menu(ui: &mut Ui, ctx: WindowCtx<'_>, out: &mut Requests) {
    let (open, app) = (ctx.open(), ctx.app());
    let selected = !open.document.main_view.selected.is_empty();
    Dropdown { label: "Edit" }.show(ui, app.theme(), |ui, popup| {
        if item(ui, popup, "Undo", Some(shortcuts::UNDO), open.can_undo()) {
            out.push_app(AppCommand::Undo);
        }
        if item(ui, popup, "Redo", Some(shortcuts::REDO), open.can_redo()) {
            out.push_app(AppCommand::Redo);
        }
        MenuSeparator::new().show(ui);
        if item(ui, popup, "Duplicate", Some(shortcuts::DUPLICATE), selected) {
            out.extend_graph(GraphIntent::duplicate(
                &open.document,
                app.library(),
                IncomingWires::Drop,
                || NodeId::unique().as_u128(),
            ));
        }
        if item(ui, popup, "Delete", Some(shortcuts::DELETE), selected) {
            out.extend_graph(GraphIntent::remove_selection(&open.document.main_view));
        }
    });
}

/// The run and the camera: what the canvas toolbar does, by name and with its keys.
fn graph_menu(ui: &mut Ui, ctx: WindowCtx<'_>, out: &mut Requests) {
    let app = ctx.app();
    let activity = app.run_state().activity();
    let selected = !ctx.document().main_view.selected.is_empty();
    Dropdown { label: "Graph" }.show(ui, app.theme(), |ui, popup| {
        let running = activity.is_executing();
        if item(ui, popup, "Run", Some(shortcuts::RUN), !running) {
            out.push_app(AppCommand::Run(RunCommand::Once));
        }
        if item(ui, popup, "Cancel Run", None, running) {
            out.push_app(AppCommand::Run(RunCommand::Cancel));
        }
        let (events, command) = if activity.event_loop_active() {
            ("Stop Events", RunCommand::StopEvents)
        } else {
            ("Start Events", RunCommand::StartEvents)
        };
        if item(ui, popup, events, None, true) {
            out.push_app(AppCommand::Run(command));
        }
        MenuSeparator::new().show(ui);
        let framings = [
            (
                "Reset View",
                Some(shortcuts::RESET_VIEW),
                Framing::Reset,
                true,
            ),
            ("Show All", None, Framing::ShowAll, true),
            ("Show Selected", None, Framing::ShowSelected, selected),
        ];
        for (label, shortcut, framing, enabled) in framings {
            if item(ui, popup, label, shortcut, enabled) {
                out.push_app(AppCommand::View(ViewCommand::Frame(framing)));
            }
        }
    });
}

fn view_menu(ui: &mut Ui, theme: &Theme, out: &mut Requests) {
    Dropdown { label: "View" }.show(ui, theme, |ui, popup| {
        if item(ui, popup, "Copy View as Image", None, true) {
            out.push_app(AppCommand::View(ViewCommand::CopyImage));
        }
        if item(ui, popup, "Save View as PNG…", None, true) {
            out.push_app(AppCommand::View(ViewCommand::SaveImage));
        }
    });
}

/// The open document's file name, or "Untitled" before its first save, with a dot while it has
/// unsaved edits; its folder is the tooltip.
fn document_name(ui: &mut Ui, ctx: WindowCtx<'_>) {
    let (open, theme) = (ctx.open(), ctx.app().theme());
    let path = open.path.as_deref();
    let name = path
        .and_then(|path| path.file_name())
        .map_or_else(|| "Untitled".into(), |name| name.to_string_lossy());
    let row = Panel::hstack()
        .id_salt("menu_bar_document")
        .size((Sizing::HUG, Sizing::HUG))
        .align(Align::CENTER)
        .gap(theme.space.sm)
        .child_align(Align::v(VAlign::Center))
        .sense(Sense::HOVER)
        .show(ui, |ui| {
            if open.dirty {
                Panel::zstack()
                    .id_salt("menu_bar_dirty")
                    .size((Sizing::fixed(DIRTY_DOT), Sizing::fixed(DIRTY_DOT)))
                    .chrome(Chrome::rounded(
                        theme.colors.text_muted,
                        Corners::all(DIRTY_DOT * 0.5),
                    ))
                    .show(ui, |_| {});
            }
            let style = support::sized_text(ui, theme.text.body);
            Text::new(fmt!(ui, "{name}")).style(&style).show(ui);
        })
        .response
        .snapshot();
    let folder = path
        .and_then(|path| path.parent())
        .map(|folder| fmt!(ui, "{}", folder.display()));
    support::tooltip_after(ui, &row, folder);
}

/// Side of the unsaved-edits dot before the document's name.
const DIRTY_DOT: f32 = 7.0;

#[cfg(test)]
mod tests {
    use palantir::{InspectedWidget, Shortcut};

    use crate::core::document::internals::DocumentFixture;
    use crate::core::document::item_id::ItemId;
    use crate::core::edit::graph_intent::GraphIntent;
    use crate::gui::app::session::internals::SessionHarness;
    use crate::gui::frame::app_command::{AppCommand, FileCommand, RunCommand, ViewCommand};
    use crate::gui::frame::shortcuts;
    use crate::gui::pane::graph_ui::gesture::pan_zoom::Framing;

    /// The visible widget whose text starts with `label`: a menu row's text is its label, then
    /// its chord.
    fn widget(h: &mut SessionHarness, label: &str) -> InspectedWidget {
        let mut listed = Vec::new();
        h.ui.ui().inspect(&mut listed);
        listed
            .into_iter()
            .find(|widget| widget.text == label || widget.text.starts_with(&format!("{label} ")))
            .unwrap_or_else(|| panic!("no widget reads {label:?}"))
    }

    /// Open `menu`, then pick `row`; the commands the pick raised.
    fn pick(h: &mut SessionHarness, menu: &str, row: &str) -> Vec<AppCommand> {
        h.prime(2);
        let trigger = widget(h, menu).id;
        h.ui.click_on(trigger);
        h.frame();
        h.prime(2);
        let row = widget(h, row).id;
        h.ui.click_on(row);
        h.frame()
    }

    /// Each row raises the command its chord's handler raises, and shows that chord.
    #[test]
    fn each_row_raises_its_command_and_shows_its_chord() {
        // The chord's label is the platform's own: Ctrl+S here, ⌘S on macOS.
        let cases: [(&str, &str, Option<Shortcut>); 6] = [
            ("File", "Save", Some(shortcuts::SAVE)),
            ("File", "Open…", Some(shortcuts::OPEN)),
            ("Edit", "Undo", Some(shortcuts::UNDO)),
            ("Graph", "Run", Some(shortcuts::RUN)),
            ("Graph", "Reset View", Some(shortcuts::RESET_VIEW)),
            ("Graph", "Show All", None),
        ];
        for (menu, row, chord) in cases {
            let mut h = SessionHarness::new(DocumentFixture::probes(1));
            if row == "Undo" {
                let node_id = h.session.open.document.graph.iter().next().unwrap().node_id;
                let _relayout = h.apply(GraphIntent::SetSelection {
                    to: [ItemId::Node(node_id)].into(),
                });
            }
            let commands = pick(&mut h, menu, row);
            let expected = match row {
                "Save" => AppCommand::File(FileCommand::Save),
                "Open…" => AppCommand::File(FileCommand::Open),
                "Undo" => AppCommand::Undo,
                "Run" => AppCommand::Run(RunCommand::Once),
                "Reset View" => AppCommand::View(ViewCommand::Frame(Framing::Reset)),
                _ => AppCommand::View(ViewCommand::Frame(Framing::ShowAll)),
            };
            assert_eq!(
                format!("{commands:?}"),
                format!("{:?}", [expected]),
                "{menu} ▸ {row}"
            );
            let trigger = widget(&mut h, menu).id;
            h.ui.click_on(trigger);
            h.frame();
            h.prime(2);
            let text = widget(&mut h, row).text;
            let shown = match chord {
                Some(chord) => format!("{row} {chord}"),
                None => row.to_owned(),
            };
            assert_eq!(text, shown, "{menu} ▸ {row} shows its chord");
        }
    }

    /// Undo is greyed out on a fresh document and live after one edit.
    #[test]
    fn undo_waits_for_an_edit() {
        let mut h = SessionHarness::new(DocumentFixture::probes(1));
        h.prime(2);
        let edit = widget(&mut h, "Edit").id;
        h.ui.click_on(edit);
        h.frame();
        h.prime(2);
        assert!(widget(&mut h, "Undo").disabled, "nothing to undo yet");
        assert!(widget(&mut h, "Delete").disabled, "nothing selected");

        let node_id = h.session.open.document.graph.iter().next().unwrap().node_id;
        let _relayout = h.apply(GraphIntent::SetSelection {
            to: [ItemId::Node(node_id)].into(),
        });
        h.prime(2);
        assert!(
            !widget(&mut h, "Undo").disabled,
            "the selection is an entry"
        );
        assert!(!widget(&mut h, "Delete").disabled, "one node selected");
    }
}
