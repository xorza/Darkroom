use std::cmp::Ordering;
use std::collections::HashMap;

use glam::Vec2;
use palantir::{
    Configure, MenuItem, Panel, PopupHandle, Scroll, Sizing, Spacing, Text, TextEdit, Tooltip, Ui,
    WidgetId,
};
use scenarium::Func;
use scenarium::NodeId;
use scenarium::{Node, NodeKind};
use scenarium::{SPECIAL_NODES, SpecialNode};

use crate::core::document::PortRef;
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::canvas::outer_canvas_widget_id;
use crate::gui::pane::graph::ctx::CanvasCtx;
use crate::gui::pane::graph::gesture::canvas_gesture::CanvasGesture;
use crate::gui::pane::graph::paint::anchored_menu::AnchoredMenu;
use crate::gui::requests::Requests;

/// One row of a category's palette list: a library `Func` or a built-in
/// special node. Collecting them into one type lets a category's rows be
/// sorted by name into a single alphabetical list.
#[derive(Debug)]
enum PaletteEntry<'a> {
    Func(&'a Func),
    Special(SpecialNode),
}

impl<'a> PaletteEntry<'a> {
    /// Borrowed from the palette's sources rather than from `self`, so a row
    /// can be grouped under its own category and moved into that group in one
    /// step.
    fn name(&self) -> &'a str {
        match *self {
            PaletteEntry::Func(f) => &f.name,
            PaletteEntry::Special(s) => &s.func().name,
        }
    }

    fn category(&self) -> &'a str {
        match *self {
            PaletteEntry::Func(f) => &f.category,
            PaletteEntry::Special(s) => &s.func().category,
        }
    }
}

/// Right-click or double-click on empty canvas → popup that lists every
/// `Func` the context's library holds plus the built-in specials, grouped by
/// category. Clicking an entry emits the intent that adds it at the click's
/// world position (inner-canvas pre-transform). Outside-click and Esc
/// dismiss.
#[derive(Default, Debug)]
pub(crate) struct NewNodeUi {
    menu: AnchoredMenu,
    /// Inner-canvas pre-transform position of the current open — the
    /// spawned node lands exactly under the click. Set at open, read at pick.
    world_pos: Vec2,
    /// Source port when a connection dropped on empty canvas opened the
    /// palette; on pick the wire resumes floating from it (rather than
    /// auto-attaching). `None` for a plain RMB / double-click. Set at open,
    /// read at pick.
    source: Option<PortRef>,
    /// Set when a node was picked from a popup that a dropped connection
    /// opened: the wire's source port, handed back to `ConnectionUI` so the
    /// wire resumes *floating* and the user clicks the exact port to land
    /// it. Taken by the canvas next frame.
    resume_floating: Option<PortRef>,
    /// The palette's search box. Cleared on each open; case-insensitively
    /// filters the listed entries by name (a matching category name shows
    /// that whole column). Empty ⇒ everything shows.
    search: Search,
}

/// The palette's search text and its case-folded copy.
///
/// Together, because the fold is derived from the text and every frame
/// needs it: keeping the buffer beside its source lets the fold reuse one
/// allocation instead of building a fresh `String` per frame.
#[derive(Default, Debug)]
struct Search {
    text: String,
    folded: String,
}

impl Search {
    fn fold(&mut self) {
        self.folded.clear();
        self.folded
            .extend(self.text.chars().flat_map(char::to_lowercase));
    }
}

impl NewNodeUi {
    /// Close the palette and drop the pending wire handoffs. The search
    /// buffers are cleared in place, keeping the allocation the fold reuses.
    pub(crate) fn reset(&mut self) {
        self.menu.reset();
        self.source = None;
        self.resume_floating = None;
        self.search.text.clear();
        self.search.folded.clear();
    }

    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        cx: CanvasCtx<'_>,
        pending_source: Option<PortRef>,
        out: &mut Requests,
    ) {
        let graph_ctx = cx.graph_ctx();
        let resp = ui.response_for(outer_canvas_widget_id());
        // Open the palette either from a bare RMB / double-click (`NewNode`
        // gesture) or from a connection dropped on empty canvas
        // (`pending_source`). Placement is the same — under the pointer.
        let mut just_opened = false;
        if (pending_source.is_some() || cx.gesture() == Some(CanvasGesture::NewNode))
            && let (Some(local), Some(rect)) = (resp.pointer_local, resp.rect)
        {
            self.world_pos = graph_ctx.viewport().to_world(local);
            self.source = pending_source;
            self.menu.open_at(rect.min + local);
            // Fresh open: empty the filter, read the graph's own
            // definitions once, and focus the search field this frame so
            // the user can type straight away.
            self.search.text.clear();
            just_opened = true;
        }

        // Everything below is per-open work — the height arithmetic reads
        // the display and a last-frame rect — so nothing but an open
        // palette on *this* pane should pay for it. `show` would answer
        // `None` anyway, but only after its arguments were built.
        if !self.menu.open_on() {
            return;
        }

        // Cap the palette height to the window so a short window scrolls
        // the overflow (via the inner vertical `Scroll`) instead of
        // running off-screen. The popup's `max_size` height bounds the
        // whole popup; the search row sits above a `Scroll` whose own cap
        // (`max_height` minus the chrome above it) keeps it from eating the
        // header's space — a `Hug` scroll otherwise claims the full cap.
        let surface = ui.display().logical_rect();
        let max_height = graph_ctx
            .theme()
            .new_node_popup_max_height
            .min(surface.size.h - 16.0)
            .max(120.0);
        let scroll_cap = (max_height - chrome_above_results(ui)).max(MIN_RESULTS_HEIGHT);
        let search = &mut self.search;
        // Rows are picked at the position the *open* captured, not wherever
        // the pointer has drifted to by the frame of the click.
        let pos = self.world_pos;
        // Inside the body, so the per-frame read only happens on the frames
        // the palette is actually up — `show` skips the body when closed.
        let chosen = self
            .menu
            .show(ui, "new_node_popup", Some(max_height), |ui, popup| {
                let palette = Palette { graph_ctx, pos };
                palette_body(ui, popup, &palette, search, scroll_cap, just_opened)
            });

        if let Some(intent) = chosen {
            out.push_graph(intent);
            // If a dropped connection opened this popup, hand its source
            // back so the wire resumes floating — the user then clicks the
            // exact port to land it, rather than it auto-attaching.
            self.resume_floating = self.source;
        }
    }

    /// Take the source of a wire whose drop spawned a node this frame — the
    /// canvas re-floats it on `ConnectionUI`. `None` on a plain palette open.
    pub(crate) fn take_resume_floating(&mut self) -> Option<PortRef> {
        self.resume_floating.take()
    }
}

/// Gap (px) below the search field, before the results scroll.
const SEARCH_ROW_GAP: f32 = 8.0;

/// Floor under the results area, so a window short enough that the chrome eats
/// the whole cap still shows a scrollable strip of rows rather than nothing.
const MIN_RESULTS_HEIGHT: f32 = 80.0;

/// Stable id for the palette's search field. A free function rather than a
/// `const` because the height cap below reads the field's rect *before* the
/// body that records it runs, so both sites have to name the same id.
pub(crate) fn search_field_wid() -> WidgetId {
    WidgetId::from_hash("new_node_search")
}

/// Stable id for the scrolling results area, the half [`chrome_above_results`]
/// sizes. Explicit like the search field's so the two ends of that arithmetic
/// are both addressable.
pub(crate) fn results_wid() -> WidgetId {
    WidgetId::from_hash("new_node_results")
}

/// Vertical space the palette's chrome claims above the scrolling results: the
/// popup's own padding, the gutter between its two children, the search field,
/// and [`SEARCH_ROW_GAP`] under it.
///
/// The inner `Scroll` needs this subtracted from the popup's height cap
/// because a stack hands every non-`Fill` child its *full* main extent — a
/// `Hug` scroll offered the whole cap takes it, and the search row above then
/// pushes the popup past the cap. `Sizing::FILL` is not the way out: palantir
/// clears a scroll's fit flag on any axis the caller didn't `Hug`, so a filled
/// scroll reports zero desired height and the popup collapses onto its search
/// row.
///
/// Every term is read rather than assumed — the field's height off its own
/// last-frame rect, the rest off the theme slot the popup is built from — so
/// restyling the field's text or the menu's padding resizes the results area
/// with it instead of silently mis-sizing the scroll. Only the first frame of
/// the first open has no rect yet and falls back to one line of body text.
fn chrome_above_results(ui: &Ui) -> f32 {
    let menu = &ui.theme().context_menu;
    // An arranged rect is margin-inclusive, so the field's already carries
    // [`SEARCH_ROW_GAP`]; only the bare-line fallback has to add it.
    let row = ui.response_for(search_field_wid()).layout_rect.map_or_else(
        || {
            let text = &ui.theme().text;
            text.line_height_for(text.font_size_px) + SEARCH_ROW_GAP
        },
        |rect| rect.size.h,
    );
    menu.padding.vert() + menu.gap + row
}

/// Record the palette: a search field pinned at the top, then the category
/// columns (one `hstack` column per category) inside a vertical `Scroll`.
/// Entries whose name (case-insensitively) contains `query` show; a category
/// whose *own* name matches shows its whole column. Empty `query` ⇒ all show.
///
/// The `Scroll` pans the overflow when the window is too short. It carries an
/// explicit `max_size` (`scroll_cap`) rather than leaning on the popup's cap:
/// a `Hug` scroll in a capped popup otherwise claims the full cap and spills
/// over the search row. `focus` grabs the field on the opening frame so the
/// user types immediately. The pick is owned, holding no `library` borrow
/// past the body.
fn palette_body(
    ui: &mut Ui,
    popup: &PopupHandle,
    palette: &Palette<'_>,
    search: &mut Search,
    scroll_cap: f32,
    focus: bool,
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
        .style(&palette.graph_ctx.theme().inline_rename.text_edit)
        .size((Sizing::fill(1.0), Sizing::HUG))
        .min_size((200.0, 0.0))
        .margin(Spacing::new(0.0, 0.0, 0.0, SEARCH_ROW_GAP))
        .show(ui);
    if focus {
        ui.request_focus(Some(search_id));
    }
    // Folded after the field records, so it reflects this frame's typing.
    search.fold();

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
                    for column in palette.columns(&search.folded) {
                        if let Some(picked) = column.show(ui, popup, palette) {
                            chosen = Some(picked);
                        }
                    }
                });
        });
    chosen
}

/// Everything one palette open lists, plus where a pick lands. Built once
/// per body from state the whole body shares, so the column and row helpers
/// take one borrow rather than four threaded parameters.
#[derive(Debug)]
struct Palette<'a> {
    graph_ctx: GraphCtx<'a>,
    /// World position the open captured — every intent a row raises places
    /// its node here.
    pos: Vec2,
}

/// One category's rows, ready to record.
#[derive(Debug)]
struct PaletteColumn<'a> {
    category: &'a str,
    entries: Vec<PaletteEntry<'a>>,
}

impl<'a> Palette<'a> {
    /// Every row the palette can list, in no particular order: the library's
    /// funcs, then the built-in specials.
    fn entries(&'a self) -> impl Iterator<Item = PaletteEntry<'a>> {
        self.graph_ctx
            .library()
            .funcs()
            .map(PaletteEntry::Func)
            .chain(SPECIAL_NODES.iter().copied().map(PaletteEntry::Special))
    }

    /// The columns to record: every category holding a matching row, sorted
    /// by name, each column's rows sorted by name too.
    ///
    /// One grouping pass over every source, rather than re-scanning them per
    /// category. A matching *category* name reveals that whole column;
    /// otherwise each row is filtered by its own name.
    fn columns(&'a self, query_lc: &str) -> Vec<PaletteColumn<'a>> {
        let mut by_category: HashMap<&str, Vec<PaletteEntry<'a>>> = HashMap::new();
        for entry in self.entries() {
            by_category.entry(entry.category()).or_default().push(entry);
        }
        let mut columns: Vec<PaletteColumn<'a>> = by_category
            .into_iter()
            .filter_map(|(category, mut entries)| {
                if !name_matches(category, query_lc) {
                    entries.retain(|entry| name_matches(entry.name(), query_lc));
                }
                if entries.is_empty() {
                    return None;
                }
                // Case-insensitive by comparison, not by key: the palette
                // re-sorts every frame it's up, and `sort_by_cached_key` with
                // a `to_lowercase()` key allocated one `String` per library
                // entry per frame to answer a question `char`-wise folding
                // answers in place.
                entries.sort_by(|a, b| lowercase_cmp(a.name(), b.name()));
                Some(PaletteColumn { category, entries })
            })
            .collect();
        // Same fold the rows above use: a category the user spelled in
        // lowercase belongs among its peers, not after every capitalized one.
        columns.sort_by(|a, b| lowercase_cmp(a.category, b.category));
        columns
    }
}

impl PaletteColumn<'_> {
    /// Record this column: its category name above its rows.
    fn show(self, ui: &mut Ui, popup: &PopupHandle, palette: &Palette<'_>) -> Option<GraphIntent> {
        let category = self.category;
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
                        for entry in self.entries {
                            if let Some(picked) = entry.show(ui, popup, palette) {
                                chosen = Some(picked);
                            }
                        }
                    });
            });
        chosen
    }
}

/// Case-insensitive ordering of two palette row names, without materializing
/// a folded copy of either. Falls back to the raw order for names that fold
/// to the same thing, so the sort stays total.
fn lowercase_cmp(a: &str, b: &str) -> Ordering {
    let folded = a
        .chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase));
    folded.then_with(|| a.cmp(b))
}

/// Case-insensitive substring match used by the palette search. An empty
/// (already-lowercased) query matches everything.
///
/// ASCII names — every built-in one — compare in place; a name carrying
/// non-ASCII falls back to a folded copy, so the match stays Unicode-correct
/// for a graph the user named themselves.
fn name_matches(name: &str, query_lc: &str) -> bool {
    if query_lc.is_empty() {
        return true;
    }
    if !name.is_ascii() {
        return name.to_lowercase().contains(query_lc);
    }
    // Only the name folds. Folding the query here too would quietly accept a
    // caller that forgot to, and the non-ASCII branch above can't.
    let (name, query) = (name.as_bytes(), query_lc.as_bytes());
    name.windows(query.len()).any(|window| {
        window
            .iter()
            .zip(query)
            .all(|(byte, folded)| byte.to_ascii_lowercase() == *folded)
    })
}

impl PaletteEntry<'_> {
    /// Record this row and, on click, the intent it raises.
    ///
    /// The three graph-shaped rows differ only in what the document has to
    /// resolve: a library graph brings the localized copy, one of this
    /// graph's own definitions is named by id, and neither builds bindings
    /// here — the commit gate seeds them off the definition it resolves.
    fn show(self, ui: &mut Ui, popup: &PopupHandle, palette: &Palette<'_>) -> Option<GraphIntent> {
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
    popup: &PopupHandle,
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
fn menu_row(ui: &mut Ui, popup: &PopupHandle, func: &Func) -> bool {
    let resp = MenuItem::new(&func.name).show(ui, popup);
    let clicked = resp.left.clicked();
    if let Some(desc) = &func.description {
        Tooltip::on(&resp.snapshot()).text(desc).show(ui);
    }
    clicked
}

#[cfg(test)]
mod tests;
