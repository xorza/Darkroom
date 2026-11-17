use palantir::{
    Align, Configure, Grid, MenuBar, MenuItem, MenuSeparator, Panel, Sense, SizeSpec, Text,
    TextWrap, Track, Ui, VAlign, WidgetId, fmt,
};

use crate::core::io::preferences::recent_documents::RecentDocuments;
use crate::gui::command::menu::{self, MENU_BAR, MenuRow, Unavailable};
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::doc_ctx::DocCtx;
use crate::gui::frame::requests::Requests;
use crate::gui::frame::stage::Stage;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::widgets::support;

/// Top-of-window menu bar: palantir's [`MenuBar`] at the left, and the document's name, with a
/// dot while it has unsaved edits, centered on the window while the menus leave it room, pushed
/// right against them where they do not, and cut in its middle where even that is too narrow.
/// A grid of a fill, a hug and a fill track (CSS's `1fr auto 1fr`): each fill keeps its content's
/// floor, so the menus widen theirs and the name moves over. Refused: a name centered in the
/// window over the menus, which it overlapped in a narrow window.
///
/// The menus are [`MENU_BAR`]'s, each row a [`Command`](crate::gui::command::Command) raised
/// against the active graph: a row an unavailable command greys keeps its place.
pub(crate) fn show(ui: &mut Ui, stage: &mut Stage<'_, DocCtx<'_>>, recent: &RecentDocuments) {
    let ctx = stage.ctx;
    let active = GraphCtx::of(ctx, ctx.active(), ui.display());
    Grid::new()
        .id(bar_wid())
        .size(SizeSpec::FILL_W)
        .cols([Track::FILL, Track::HUG, Track::FILL])
        .rows([Track::HUG])
        .show(ui, |ui| {
            MenuBar::new()
                .id(menus_wid())
                .grid_cell((0, 0))
                .size(SizeSpec::HUG)
                .align(Align::LEFT)
                .show(ui, |bar| {
                    for spec in &MENU_BAR {
                        bar.menu(spec.title, |ui| {
                            // The rows around each recent-documents marker, the list between.
                            let mut parts = spec
                                .rows
                                .split(|row| matches!(row, MenuRow::RecentDocuments));
                            let first = parts.next().expect("a split yields a first part");
                            menu::record(ui, first, active, stage.out, Unavailable::Greyed);
                            for rest in parts {
                                MenuSeparator::new().show(ui);
                                if recent_rows(ui, recent, stage.out) {
                                    MenuSeparator::new().show(ui);
                                }
                                menu::record(ui, rest, active, stage.out, Unavailable::Greyed);
                            }
                        });
                    }
                });
            document_name(ui, ctx);
        });
}

/// One row a recent document, its file's name with its folder as the tooltip; a pick opens it
/// through the unsaved-changes prompt. Whether any was listed.
fn recent_rows(ui: &mut Ui, recent: &RecentDocuments, out: &mut Requests) -> bool {
    let mut any = false;
    for (idx, path) in recent.iter().enumerate() {
        any = true;
        let name = path.file_name().unwrap_or(path.as_os_str());
        let label = fmt!(ui, "{}", name.display());
        // By its place, as two folders may hold one name.
        let row = MenuItem::new(label).id_salt(("recent", idx)).show(ui);
        if row.clicked() {
            out.push_app(AppCommand::OpenAt(path.to_path_buf()));
        }
        let folder = path.parent().map(|folder| fmt!(ui, "{}", folder.display()));
        support::tooltip_after(ui, &row, folder);
    }
    any
}

/// The open document's file name, or "Untitled" before its first save, with a dot while it has
/// unsaved edits; its folder is the tooltip.
fn document_name(ui: &mut Ui, ctx: DocCtx<'_>) {
    let (open, theme) = (ctx.open(), ctx.theme());
    let path = open.path();
    let name = path
        .and_then(|path| path.file_name())
        .map_or_else(|| "Untitled".into(), |name| name.to_string_lossy());
    let row = Panel::hstack()
        .id(document_wid())
        .grid_cell((0, 1))
        .size(SizeSpec::HUG)
        .align(Align::CENTER)
        .gap(theme.space.sm)
        .child_align(VAlign::Center)
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
    use std::path::{Path, PathBuf};

    use glam::UVec2;
    use palantir::{InspectedWidget, Key};
    use scenarium::{Compound, CompoundId, GraphId, Interface};

    use crate::core::document::internals::{DocumentFixture, insert_compound};
    use crate::core::document::item_id::ItemId;
    use crate::core::document::tab_ref::TabRef;
    use crate::core::document::viewport::Viewport;
    use crate::core::edit::graph_intent::GraphIntent;
    use crate::gui::app::session::internals::SessionHarness;
    use crate::gui::command::menu::{MENU_BAR, MenuRow};
    use crate::gui::command::{Availability, Command};
    use crate::gui::frame::app_command::AppCommand;
    use crate::gui::frame::app_ctx::AppCtx;
    use crate::gui::frame::doc_ctx::DocCtx;
    use crate::gui::graph_ctx::GraphCtx;
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

    /// `debug` with each `Id(..)` emptied: two fixtures mint ids of their own.
    fn without_ids(debug: &str) -> String {
        let mut out = String::new();
        let mut rest = debug;
        while let Some(start) = rest.find("Id(") {
            let end = start + rest[start..].find(')').expect("an id closes");
            out.push_str(&rest[..start]);
            out.push_str("Id()");
            rest = &rest[end + 1..];
        }
        out.push_str(rest);
        out
    }

    /// What an edit leaves that two fixtures with ids of their own can be compared by: how many
    /// nodes and how many selected items the main graph holds, where its camera looks, and
    /// which tabs are open.
    fn shape(h: &SessionHarness) -> (usize, usize, Viewport, Vec<TabRef>) {
        let document = h.session.open.document();
        (
            document.graphs.main().len(),
            document.main_view.selected.len(),
            document.main_view.viewport,
            document.layout.all_tabs().collect(),
        )
    }

    /// Each row shows its command's label and chord, greyed exactly where the command is
    /// unavailable, and a pick raises what the command's chord raises: the same commands for the
    /// shell, and the same document.
    #[test]
    fn each_row_is_its_command() {
        // One node, selected and copied, so the selection's commands and Paste are live.
        let fixture = || {
            let mut h = SessionHarness::new(DocumentFixture::probes(1));
            let node_id = h
                .session
                .open
                .document()
                .graphs
                .main()
                .iter()
                .next()
                .unwrap()
                .node_id;
            h.apply(GraphIntent::SetSelection {
                to: [ItemId::Node(node_id)].into(),
            });
            h.prime(2);
            h
        };
        let available = |h: &mut SessionHarness, command: Command| {
            let display = h.ui.ui_mut().display();
            let app = AppCtx::new(
                &h.theme,
                &h.library,
                &h.run_state,
                &h.shared,
                h.preferences.run_mode,
            );
            let doc = DocCtx::new(app, &h.session.open);
            let ctx = GraphCtx::of(doc, GraphId::Main, display);
            command.availability(ctx) != Availability::Unavailable
        };
        let mut swept = 0;
        for spec in &MENU_BAR {
            for row in spec.rows {
                let MenuRow::Command(command) = *row else {
                    continue;
                };
                swept += 1;
                let label = command.spec().label;
                let mut picked = fixture();
                let live = available(&mut picked, command);
                let trigger = widget(&picked, spec.title).id;
                picked.ui.click(trigger);
                picked.frame();
                picked.prime(2);
                let shown = widget(&picked, label);
                let text = match command.keys() {
                    Some(keys) => format!("{label} {keys}"),
                    None => label.to_owned(),
                };
                assert_eq!(shown.text, text, "{label} shows its chord");
                assert_eq!(shown.disabled, !live, "{label} greys with its command");
                let Some(keys) = command.keys().filter(|_| live) else {
                    continue;
                };
                picked.ui.click(shown.id);
                let by_pick = picked.frame();
                let mut pressed = fixture();
                pressed.ui.key(keys);
                let by_chord = pressed.frame();
                // An undo by its chord applies in the frame's first phase, and by its row after
                // the pass, where the harness runs no command: its document is the chord's work.
                if !matches!(command, Command::Undo) {
                    assert_eq!(
                        without_ids(&format!("{by_pick:?}")),
                        without_ids(&format!("{by_chord:?}")),
                        "{label}: the row and the chord raise one thing"
                    );
                    assert_eq!(
                        shape(&picked),
                        shape(&pressed),
                        "{label}: the row and the chord edit alike"
                    );
                }
            }
        }
        // File 7, Edit 11, View 10, Graph 13, Help 1.
        assert_eq!(swept, 42, "every command row of the bar");
    }

    /// File lists each recent document by its file's name, newest first, and a pick asks the
    /// shell to open that path.
    #[test]
    fn a_recent_document_opens_by_its_row() {
        let mut h = SessionHarness::new(DocumentFixture::probes(1));
        h.preferences
            .recent
            .note(Path::new("/work/a/first.darkroom"));
        h.preferences
            .recent
            .note(Path::new("/work/b/second.darkroom"));
        h.prime(2);
        let file = widget(&h, "File").id;
        h.ui.click(file);
        h.frame();
        h.prime(2);
        let (first, second) = (widget(&h, "first.darkroom"), widget(&h, "second.darkroom"));
        assert!(second.rect.min.y < first.rect.min.y, "newest first");
        h.ui.click(first.id);
        let commands = h.frame();
        assert_eq!(
            format!("{commands:?}"),
            format!(
                "{:?}",
                [AppCommand::OpenAt(PathBuf::from("/work/a/first.darkroom"))]
            )
        );
    }

    /// Export offers the compound a selected instance places, and waits for one on the main
    /// graph with nothing picked.
    #[test]
    fn export_names_the_selected_instance() {
        let mut fixture = DocumentFixture::default();
        let id = CompoundId::from_u128(1);
        insert_compound(
            &mut fixture.document,
            id,
            Compound::new("inner", Interface::default()),
        );
        let instance = fixture.add_instance(id);
        let mut h = SessionHarness::new(fixture);
        h.prime(2);
        let graph = widget(&h, "Graph").id;
        h.ui.click(graph);
        h.frame();
        h.prime(2);
        assert!(widget(&h, "Export Compound…").disabled, "nothing picked");
        h.ui.key(Key::Escape);
        h.frame();

        h.apply(GraphIntent::SetSelection {
            to: [ItemId::Node(instance)].into(),
        });
        let commands = pick(&mut h, "Graph", "Export Compound…");
        assert_eq!(
            format!("{commands:?}"),
            format!("{:?}", [AppCommand::Export { compound: id }])
        );
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
            .graphs
            .main()
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

    /// At 1200 px the name's center is the bar's; at 680 px the even split, (680 − name) / 2, is
    /// under the menus' width, so the name starts where the menus end, at its full width; at 470
    /// px it has less than that width past the menus, so it is cut to fit there, centered in it.
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

        let [bar, menus, name] = at(680);
        assert!((bar.size.w - full) / 2.0 < menus.size.w);
        assert!(menus.size.w + full <= bar.size.w);
        assert_eq!(menus.min.x, bar.min.x);
        assert_eq!(name.min.x, menus.max().x, "moved against the menus");
        assert_eq!(name.size.w, full, "whole");

        let [bar, menus, name] = at(470);
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
