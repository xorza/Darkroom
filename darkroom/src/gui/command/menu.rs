//! The rows of each menu, and the one recorder that draws them.
//!
//! - **Five menus: File, Edit, View, Graph, Help.** View takes the camera and the panes, each
//!   pane checked while it is open, as the Windows guidelines place there the commands that
//!   change the view and not the data
//!   ([menus](https://learn.microsoft.com/en-us/windows/win32/uxguide/cmd-menus)).
//! - **A node's menu is the selection's commands**, each with its chord, as the same guidelines
//!   ask a shortcut menu to repeat the Edit and View rows of the object it opens on.
//! - **Recent documents list in File**, between Open and Save. Refused: a submenu, which palantir
//!   has none of, for a list of ten.

use palantir::{Configure, MenuItem, MenuSeparator, Ui};

use crate::core::document::pane_kind::PaneKind;
use crate::gui::command::{Availability, Command};
use crate::gui::frame::requests::Requests;
use crate::gui::graph_ctx::GraphCtx;

/// One row of a menu.
#[derive(Clone, Copy, Debug)]
pub(crate) enum MenuRow {
    Command(Command),
    Separator,
    /// Where the bar lists the recent documents, between two separators of its own: one row
    /// each, and nothing for none. The bar records them, as only it holds the list.
    RecentDocuments,
}

/// A menu of the bar: its title and its rows.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MenuSpec {
    pub(crate) title: &'static str,
    pub(crate) rows: &'static [MenuRow],
}

/// What a menu does with a row whose command is unavailable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unavailable {
    /// Draw it greyed: a menu of the bar, whose rows hold their places.
    Greyed,
    /// Leave it out, and a separator that would then stand first, last or doubled: a menu that
    /// opens on an item and lists what that item takes.
    Hidden,
}

use Command as C;
use MenuRow::{Command as Row, Separator};

/// The menu bar, left to right.
pub(crate) const MENU_BAR: [MenuSpec; 5] = [
    MenuSpec {
        title: "File",
        rows: &[
            Row(C::NewDocument),
            Row(C::OpenDocument),
            MenuRow::RecentDocuments,
            Row(C::SaveDocument),
            Row(C::SaveDocumentAs),
            Separator,
            Row(C::PackResources),
            Separator,
            Row(C::ShowPane(PaneKind::Preferences)),
            Separator,
            Row(C::Quit),
        ],
    },
    MenuSpec {
        title: "Edit",
        rows: &[
            Row(C::Undo),
            Row(C::Redo),
            Separator,
            Row(C::Cut),
            Row(C::Copy),
            Row(C::Paste),
            Row(C::Duplicate),
            Row(C::DuplicateWithWires),
            Row(C::Delete),
            Separator,
            Row(C::SelectAll),
            Row(C::Rename),
        ],
    },
    MenuSpec {
        title: "View",
        rows: &[
            Row(C::ZoomIn),
            Row(C::ZoomOut),
            Row(C::ZoomToActual),
            Row(C::ZoomToFit),
            Row(C::ZoomToSelection),
            Separator,
            Row(C::ShowPane(PaneKind::Viewer)),
            Row(C::ShowPane(PaneKind::Inspector)),
            Separator,
            Row(C::CloseTab),
            Separator,
            Row(C::CopyViewImage),
            Row(C::SaveViewImage),
        ],
    },
    MenuSpec {
        title: "Graph",
        rows: &[
            Row(C::Run),
            Row(C::CancelRun),
            Row(C::RunToSelection),
            Row(C::ToggleEvents),
            Row(C::ToggleRunOnChange),
            Separator,
            Row(C::AddNode),
            Row(C::ReplaceNode),
            Separator,
            Row(C::CollapseToCompound),
            Row(C::ExpandCompound),
            Row(C::MakeUnique),
            Row(C::ExportCompound),
            Separator,
            Row(C::SelectGhosts),
            Row(C::RemoveGhosts),
            Row(C::RemoveUnusedCompounds),
        ],
    },
    MenuSpec {
        title: "Help",
        rows: &[Row(C::ShowPane(PaneKind::Reference))],
    },
];

/// The menu a right-click on a node opens: the selection's commands.
pub(crate) const NODE_MENU: &[MenuRow] = &[
    Row(C::RunToSelection),
    Separator,
    Row(C::Cut),
    Row(C::Copy),
    Row(C::Duplicate),
    Row(C::DuplicateWithWires),
    Row(C::Delete),
    Row(C::Rename),
    Separator,
    Row(C::ReplaceNode),
    Row(C::CollapseToCompound),
    Separator,
    Row(C::OpenCompound),
    Row(C::ExpandCompound),
    Row(C::MakeUnique),
    Row(C::MakeLocal),
    Row(C::ToggleLock),
    Separator,
    Row(C::ShowPane(PaneKind::Inspector)),
];

/// The camera's commands, as a pane's zoom control lists them.
/// The run chip's context menu: the runs, and when one starts unasked.
pub(crate) const RUN_MENU: &[MenuRow] = &[
    Row(C::Run),
    Row(C::RunToSelection),
    Separator,
    Row(C::ToggleRunOnChange),
];

pub(crate) const ZOOM_MENU: &[MenuRow] = &[
    Row(C::ZoomIn),
    Row(C::ZoomOut),
    Separator,
    Row(C::ZoomToActual),
    Row(C::ZoomToFit),
    Row(C::ZoomToSelection),
];

/// Record `rows`, which hold no [`MenuRow::RecentDocuments`], into the open menu, and raise the
/// command of a picked row against `ctx`, the graph the menu acts on. Each row shows its command's label and chord, checked while the
/// command is in effect.
pub(crate) fn record(
    ui: &mut Ui,
    rows: &[MenuRow],
    ctx: GraphCtx<'_>,
    out: &mut Requests,
    unavailable: Unavailable,
) {
    // A separator waits for the row after it, so one never stands first, last or doubled where
    // rows are left out.
    let mut separator_owed = false;
    let mut shown_any = false;
    for row in rows {
        let command = match *row {
            Separator => {
                separator_owed = shown_any;
                continue;
            }
            Row(command) => command,
            MenuRow::RecentDocuments => {
                unreachable!("the bar records the recent documents itself")
            }
        };
        let availability = command.availability(ctx);
        if availability == Availability::Unavailable && unavailable == Unavailable::Hidden {
            continue;
        }
        if separator_owed {
            MenuSeparator::new().show(ui);
            separator_owed = false;
        }
        shown_any = true;
        let mut item = MenuItem::new(command.spec().label)
            .disabled(availability == Availability::Unavailable)
            .selected(availability == Availability::On);
        if let Some(keys) = command.keys() {
            item = item.shortcut_hint(keys);
        }
        if item.show(ui).clicked() {
            command.raise(ui, ctx, out);
        }
    }
}
