use palantir::{
    Align, Configure, DockOperation, Grid, MenuBar, MenuBarMenus, MenuItem, MenuSeparator, Panel,
    Sense, Shortcut, Sizing, Text, TextWrap, Track, Ui, VAlign, WidgetId, fmt,
};
use scenarium::NodeId;

use crate::core::document::tab_ref::TabRef;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::incoming_wires::IncomingWires;
use crate::gui::frame::app_command::{AppCommand, FileCommand, RunCommand, ViewCommand};
use crate::gui::frame::framing::Framing;
use crate::gui::frame::record_stage::RecordStage;
use crate::gui::frame::requests::Requests;
use crate::gui::frame::shortcuts;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::widgets::support;

/// Top-of-window menu bar: palantir's [`MenuBar`] at the left, and the document's name, with a
/// dot while it has unsaved edits, centered on the window while the menus leave it room, pushed
/// right against them where they do not, and cut in its middle where even that is too narrow.
/// A grid of a fill, a hug and a fill track, CSS's `1fr auto 1fr`: the fill tracks split what
/// is left evenly, and each keeps its content's floor, so the menus widen theirs and the name
/// moves over. Refused: a name centered in the window over the menus, which it overlapped in a
/// narrow window.
///
/// A pick goes onto the stage's requests in whichever tier it belongs to: graph edits as intents,
/// the file lifecycle, the history, the run, the view and `Quit` as [`AppCommand`]s for `App` to
/// run after the pass, Preferences as the [`DockOperation`] that opens its tab. Every item with a chord
/// shows it, read from [`shortcuts`], the table its handler reads.
pub(crate) fn show(ui: &mut Ui, stage: &mut RecordStage<'_, GraphCtx<'_>>) {
    let ctx = stage.ctx;
    Grid::new()
        .id(bar_wid())
        .size((Sizing::FILL, Sizing::HUG))
        .cols([Track::FILL, Track::HUG, Track::FILL])
        .rows([Track::HUG])
        .show(ui, |ui| {
            MenuBar::new()
                .id(menus_wid())
                .grid_cell((0, 0))
                .size((Sizing::HUG, Sizing::HUG))
                .align(Align::LEFT)
                .show(ui, |bar| {
                    file_menu(bar, stage.out);
                    edit_menu(bar, stage);
                    graph_menu(bar, stage);
                    view_menu(bar, stage.out);
                });
            document_name(ui, ctx);
        });
}

/// One row, `enabled` or greyed out, with the chord its handler reads shown beside it; `true`
/// when picked.
fn item(ui: &mut Ui, label: &'static str, shortcut: Option<Shortcut>, enabled: bool) -> bool {
    let mut row = MenuItem::new(label).disabled(!enabled);
    if let Some(shortcut) = shortcut {
        row = row.shortcut_hint(shortcut);
    }
    row.show(ui).clicked()
}

fn file_menu(bar: &mut MenuBarMenus<'_>, out: &mut Requests) {
    bar.menu("File", |ui| {
        let file = [
            ("New", Some(shortcuts::NEW), FileCommand::New),
            ("Open…", Some(shortcuts::OPEN), FileCommand::Open),
            ("Save", Some(shortcuts::SAVE), FileCommand::Save),
            ("Save As…", Some(shortcuts::SAVE_AS), FileCommand::SaveAs),
        ];
        for (label, shortcut, command) in file {
            if item(ui, label, shortcut, true) {
                out.push_app(AppCommand::File(command));
            }
        }
        MenuSeparator::new().show(ui);
        if item(ui, "Preferences", None, true) {
            out.push_view(DockOperation::OpenTab {
                tab: TabRef::Preferences,
            });
        }
        MenuSeparator::new().show(ui);
        if item(ui, "Quit", Some(shortcuts::QUIT), true) {
            out.push_app(AppCommand::Quit);
        }
    });
}

/// The history and the selection's edits. Duplicate drops the wires from producers outside the
/// selection, as Ctrl+D does.
fn edit_menu(bar: &mut MenuBarMenus<'_>, stage: &mut RecordStage<'_, GraphCtx<'_>>) {
    let (ctx, out) = (stage.ctx, &mut *stage.out);
    let open = ctx.open();
    let selected = !open.document().main_view.selected.is_empty();
    bar.menu("Edit", |ui| {
        if item(ui, "Undo", Some(shortcuts::UNDO), open.can_undo()) {
            out.push_app(AppCommand::Undo);
        }
        if item(ui, "Redo", Some(shortcuts::REDO), open.can_redo()) {
            out.push_app(AppCommand::Redo);
        }
        MenuSeparator::new().show(ui);
        if item(ui, "Duplicate", Some(shortcuts::DUPLICATE), selected) {
            out.extend_graph(GraphIntent::duplicate(
                open.document(),
                ctx.library(),
                IncomingWires::Drop,
                || NodeId::unique().as_u128(),
            ));
        }
        if item(ui, "Delete", Some(shortcuts::DELETE), selected) {
            out.extend_graph(GraphIntent::remove_selection(&open.document().main_view));
        }
    });
}

/// The run and the camera: what the canvas toolbar does, by name and with its keys.
fn graph_menu(bar: &mut MenuBarMenus<'_>, stage: &mut RecordStage<'_, GraphCtx<'_>>) {
    let (ctx, out) = (stage.ctx, &mut *stage.out);
    let activity = ctx.run_state().activity();
    let selected = !ctx.document().main_view.selected.is_empty();
    bar.menu("Graph", |ui| {
        let running = activity.is_executing();
        if item(ui, "Run", Some(shortcuts::RUN), !running) {
            out.push_app(AppCommand::Run(RunCommand::Once));
        }
        if item(ui, "Cancel Run", None, running) {
            out.push_app(AppCommand::Run(RunCommand::Cancel));
        }
        let (events, command) = if activity.event_loop_active() {
            ("Stop Events", RunCommand::StopEvents)
        } else {
            ("Start Events", RunCommand::StartEvents)
        };
        if item(ui, events, None, true) {
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
            if item(ui, label, shortcut, enabled) {
                out.push_app(AppCommand::View(ViewCommand::Frame(framing)));
            }
        }
    });
}

fn view_menu(bar: &mut MenuBarMenus<'_>, out: &mut Requests) {
    bar.menu("View", |ui| {
        if item(ui, "Copy View as Image", None, true) {
            out.push_app(AppCommand::View(ViewCommand::CopyImage));
        }
        if item(ui, "Save View as PNG…", None, true) {
            out.push_app(AppCommand::View(ViewCommand::SaveImage));
        }
    });
}

/// The open document's file name, or "Untitled" before its first save, with a dot while it has
/// unsaved edits; its folder is the tooltip.
fn document_name(ui: &mut Ui, ctx: GraphCtx<'_>) {
    let (open, theme) = (ctx.open(), ctx.theme());
    let path = open.path();
    let name = path
        .and_then(|path| path.file_name())
        .map_or_else(|| "Untitled".into(), |name| name.to_string_lossy());
    let row = Panel::hstack()
        .id(document_wid())
        .grid_cell((0, 1))
        .size((Sizing::HUG, Sizing::HUG))
        .align(Align::CENTER)
        .gap(theme.space.sm)
        .child_align(Align::v(VAlign::Center))
        .sense(Sense::HOVER)
        .show(ui, |ui| {
            if open.is_dirty() {
                support::dot_cell(ui, "menu_bar_dirty", theme, theme.ink.muted);
            }
            let style = support::body_text(ui, theme);
            Text::new(fmt!(ui, "{name}"))
                .style(&style)
                .text_wrap(TextWrap::EllipsisMiddle)
                .show(ui);
        })
        .response;
    let folder = path
        .and_then(|path| path.parent())
        .map(|folder| fmt!(ui, "{}", folder.display()));
    support::tooltip_after(ui, &row, folder);
}

fn bar_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.menu_bar")
}

fn menus_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.menu_bar.menus")
}

fn document_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.menu_bar.document")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use glam::UVec2;
    use palantir::{InspectedWidget, Shortcut};

    use crate::core::document::internals::DocumentFixture;
    use crate::core::document::item_id::ItemId;
    use crate::core::edit::graph_intent::GraphIntent;
    use crate::gui::app::session::internals::SessionHarness;
    use crate::gui::frame::app_command::{AppCommand, FileCommand, RunCommand, ViewCommand};
    use crate::gui::frame::framing::Framing;
    use crate::gui::frame::shortcuts;
    use crate::gui::main_window::menu_bar::{bar_wid, document_wid, menus_wid};

    /// The visible widget whose text starts with `label`: a menu row's text is its label, then
    /// its chord.
    fn widget(h: &SessionHarness, label: &str) -> InspectedWidget {
        h.ui.widgets(Some(label))
            .into_iter()
            .find(|widget| widget.text == label || widget.text.starts_with(&format!("{label} ")))
            .unwrap_or_else(|| panic!("no widget reads {label:?}"))
    }

    /// Open `menu`, then pick `row`; the commands the pick raised.
    fn pick(h: &mut SessionHarness, menu: &str, row: &str) -> Vec<AppCommand> {
        h.prime(2);
        let trigger = widget(h, menu).id;
        h.ui.click(trigger);
        h.frame();
        h.prime(2);
        let row = widget(h, row).id;
        h.ui.click(row);
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
                let node_id = h
                    .session
                    .open
                    .document()
                    .graph
                    .iter()
                    .next()
                    .unwrap()
                    .node_id;
                h.apply(GraphIntent::SetSelection {
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
            let trigger = widget(&h, menu).id;
            h.ui.click(trigger);
            h.frame();
            h.prime(2);
            let text = widget(&h, row).text;
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
        let edit = widget(&h, "Edit").id;
        h.ui.click(edit);
        h.frame();
        h.prime(2);
        assert!(widget(&h, "Undo").disabled, "nothing to undo yet");
        assert!(widget(&h, "Delete").disabled, "nothing selected");

        let node_id = h
            .session
            .open
            .document()
            .graph
            .iter()
            .next()
            .unwrap()
            .node_id;
        h.apply(GraphIntent::SetSelection {
            to: [ItemId::Node(node_id)].into(),
        });
        h.prime(2);
        assert!(!widget(&h, "Undo").disabled, "the selection is an entry");
        assert!(!widget(&h, "Delete").disabled, "one node selected");
    }

    /// The document's name centers on the window while the menus leave it room, moves right
    /// against them where they do not, and is cut in its middle where even that is too narrow;
    /// the menus stay at the left throughout. At 1200 px the name's center is the bar's; at 600
    /// px the even split, (600 − name) / 2, is under the menus' width, so the name starts where
    /// the menus end, at its full width; at 420 px it has less than that width past the menus,
    /// so it is cut to fit there, centered in it, its last glyph's slack on both sides.
    #[test]
    fn the_name_centers_then_moves_past_the_menus_then_is_cut() {
        let mut h = SessionHarness::new(DocumentFixture::probes(1));
        h.session.open.set_path(PathBuf::from(
            "/tmp/a-document-name-long-enough-to-cut.darkroom",
        ));
        let mut at = |width: u32| {
            h.ui.resize(UVec2::new(width, 800));
            h.prime(2);
            let rect = |wid| h.ui.rect(wid).expect("recorded");
            [bar_wid(), menus_wid(), document_wid()].map(rect)
        };

        let [bar, menus, name] = at(1200);
        let full = name.size.w;
        assert_eq!(menus.min.x, bar.min.x);
        assert_eq!(name.center().x, bar.center().x, "centered on the window");
        assert!(name.min.x > menus.max().x, "clear of the menus");

        let [bar, menus, name] = at(600);
        assert!((bar.size.w - full) / 2.0 < menus.size.w);
        assert!(menus.size.w + full <= bar.size.w);
        assert_eq!(menus.min.x, bar.min.x);
        assert_eq!(name.min.x, menus.max().x, "moved against the menus");
        assert_eq!(name.size.w, full, "whole");

        let [bar, menus, name] = at(420);
        let left = bar.max().x - menus.max().x;
        assert!(left < full);
        assert_eq!(menus.min.x, bar.min.x);
        assert!(name.size.w <= left, "cut to what is left: {}", name.size.w);
        assert_eq!(
            name.center().x,
            f32::midpoint(menus.max().x, bar.max().x),
            "centered in what is left"
        );
    }
}
