//! [`NodePalette`]: everything one new-node popup lists, and the rows and
//! columns it records.

use palantir::CloseHandle;
use palantir::prelude::*;
use scenarium::Func;
use scenarium::NodeId;
use scenarium::{Node, NodeKind};
use scenarium::{SPECIAL_NODES, SpecialNode};

use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::gesture::new_node::palette_rows::{PaletteRow, PaletteRows};
use crate::gui::pane::graph::gesture::new_node::{
    SEARCH_ROW_GAP, Search, results_wid, search_field_wid,
};

/// One row of a category's palette list: a library `Func` or a built-in
/// special node. Collecting them into one type lets every source sort
/// into one list, which is what makes a category a run inside it.
#[derive(Clone, Copy, Debug)]
pub(super) enum PaletteEntry<'a> {
    Func(&'a Func),
    Special(SpecialNode),
}

impl<'a> PaletteEntry<'a> {
    /// Borrowed from the palette's sources rather than from `self`, so a
    /// name outlives any borrow of the row that yielded it.
    pub(super) fn name(self) -> &'a str {
        match self {
            PaletteEntry::Func(f) => &f.name,
            PaletteEntry::Special(s) => &s.func().name,
        }
    }

    pub(super) fn category(self) -> &'a str {
        match self {
            PaletteEntry::Func(f) => &f.category,
            PaletteEntry::Special(s) => &s.func().category,
        }
    }

    /// The identity [`PaletteRows`] keeps for this entry.
    pub(super) const fn row(self) -> PaletteRow {
        match self {
            PaletteEntry::Func(f) => PaletteRow::Func(f.id),
            PaletteEntry::Special(s) => PaletteRow::Special(s),
        }
    }
}

/// Everything one palette open lists, plus where a pick lands. Built once
/// per body from state the whole body shares, so the column and row helpers
/// take one borrow rather than four threaded parameters.
#[derive(Debug)]
pub(super) struct NodePalette<'a> {
    graph_ctx: GraphCtx<'a>,
    /// World position the open captured — every intent a row raises places
    /// its node here.
    pos: Vec2,
}

/// One category's rows, ready to record: a run inside [`PaletteRows`], not a
/// list of its own.
#[derive(Debug)]
struct PaletteColumn<'a> {
    category: &'a str,
    rows: &'a [PaletteRow],
}

impl<'a> NodePalette<'a> {
    pub(super) const fn new(graph_ctx: GraphCtx<'a>, pos: Vec2) -> Self {
        Self { graph_ctx, pos }
    }

    /// Every row the palette can list, in no particular order: the library's
    /// funcs, then the built-in specials.
    fn entries(&self) -> impl Iterator<Item = PaletteEntry<'a>> {
        self.graph_ctx
            .library()
            .funcs()
            .map(PaletteEntry::Func)
            .chain(SPECIAL_NODES.iter().copied().map(PaletteEntry::Special))
    }
}

impl PaletteColumn<'_> {
    /// Record this column: its category name above its rows.
    fn show(
        self,
        ui: &mut Ui,
        popup: &CloseHandle,
        palette: &NodePalette<'_>,
    ) -> Option<GraphIntent> {
        let category = self.category;
        let library = palette.graph_ctx.library();
        let mut chosen = None;
        Panel::vstack()
            .id_salt(("new_node_col", category))
            .size((Sizing::HUG, Sizing::HUG))
            .gap(4.0)
            .show(ui, |ui| {
                Text::new(category)
                    .id_salt(("new_node_cat", category))
                    .show(ui);
                Panel::vstack()
                    .id_salt(("new_node_funcs", category))
                    .size((Sizing::HUG, Sizing::HUG))
                    .gap(2.0)
                    .show(ui, |ui| {
                        for entry in self.rows.iter().filter_map(|row| row.entry(library)) {
                            if let Some(picked) = entry.show(ui, popup, palette) {
                                chosen = Some(picked);
                            }
                        }
                    });
            });
        chosen
    }
}

impl NodePalette<'_> {
    /// Record the search field and the rows it matches. `opened` is the
    /// frame the palette opened: it takes the focus, and filters the rows
    /// afresh against the library as it is now.
    pub(super) fn body(
        &self,
        ui: &mut Ui,
        popup: &CloseHandle,
        search: &mut Search,
        rows: &mut PaletteRows,
        scroll_cap: f32,
        opened: bool,
    ) -> Option<GraphIntent> {
        let mut chosen: Option<GraphIntent> = None;

        let search_id = search_field_wid();
        TextEdit::new(&mut search.text)
            .id(search_id)
            // The field filters the palette rather than editing a value, so Esc
            // closes the whole popup instead of blurring the only thing in it
            // the user can type into.
            .escape_falls_through()
            .placeholder("Search…")
            .style(&self.graph_ctx.theme().inline_rename.text_edit)
            .size((Sizing::fill(1.0), Sizing::HUG))
            .min_size((200.0, 0.0))
            .margin(Spacing::new(0.0, 0.0, 0.0, SEARCH_ROW_GAP))
            .show(ui);
        if opened {
            ui.set_focus(search_id);
        }
        // Folded after the field records, so it reflects this frame's typing.
        if search.fold() || opened {
            rows.refilter(self.graph_ctx.library(), self.entries(), &search.folded);
        }
        let library = self.graph_ctx.library();

        Scroll::vertical()
            .id(results_wid())
            .size((Sizing::HUG, Sizing::HUG))
            .max_size((f32::INFINITY, scroll_cap))
            .show(ui, |ui| {
                Panel::hstack()
                    .id_salt("new_node_columns")
                    .size((Sizing::HUG, Sizing::HUG))
                    .gap(12.0)
                    .show(ui, |ui| {
                        for rows in rows.columns() {
                            // A column whose funcs all left the library since
                            // the filtering has nothing to record.
                            let Some(first) = rows.iter().find_map(|row| row.entry(library)) else {
                                continue;
                            };
                            let column = PaletteColumn {
                                category: first.category(),
                                rows,
                            };
                            if let Some(picked) = column.show(ui, popup, self) {
                                chosen = Some(picked);
                            }
                        }
                    });
            });
        chosen
    }
}

impl PaletteEntry<'_> {
    /// Record this row and, on click, the intent it raises.
    ///
    /// The three graph-shaped rows differ only in what the document has to
    /// resolve: a library graph brings the localized copy, one of this
    /// graph's own definitions is named by id, and neither builds bindings
    /// here — the commit gate seeds them off the definition it resolves.
    fn show(
        self,
        ui: &mut Ui,
        popup: &CloseHandle,
        palette: &NodePalette<'_>,
    ) -> Option<GraphIntent> {
        let pos = palette.pos;
        match self {
            PaletteEntry::Func(func) => add_from_func(ui, popup, pos, func, || func.into()),
            // A special node's `Func` is hardcoded rather than
            // library-registered, so the node it spawns is a
            // `NodeKind::Special` — `Node::new` reads the same hardcoded func
            // for its name and cache mode, which is the only thing that
            // differs from a library row.
            PaletteEntry::Special(special) => add_from_func(ui, popup, pos, special.func(), || {
                Node::new(NodeKind::Special(special))
            }),
        }
    }
}

/// Record a `Func`-shaped row and, on click, the `AddNode` it raises.
///
/// Shared by the library entry and the special node: both name their node
/// after the same `Func` and seed the same default bindings from it, so
/// only the `Node` value itself differs. Built by a closure rather than
/// passed in, so an unclicked row — every row, most frames — never pays
/// for one.
fn add_from_func(
    ui: &mut Ui,
    popup: &CloseHandle,
    pos: Vec2,
    func: &Func,
    node: impl FnOnce() -> Node,
) -> Option<GraphIntent> {
    menu_row(ui, popup, func).then(|| {
        let node_id = NodeId::unique();
        GraphIntent::AddNode {
            pos,
            node_id,
            node: node(),
            bindings: func.default_bindings(node_id).collect(),
        }
    })
}

/// Record a row for `func`, hovering its description as a tooltip. The
/// tooltip has to record whether or not the row was clicked, so the click is
/// latched first.
fn menu_row(ui: &mut Ui, popup: &CloseHandle, func: &Func) -> bool {
    let resp = MenuItem::new(&func.name).show(ui, popup);
    let clicked = resp.left.clicked();
    if let Some(desc) = &func.description {
        Tooltip::on(&resp.snapshot()).label(desc).show(ui);
    }
    clicked
}
