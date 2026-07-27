use std::cmp::Ordering;
use std::collections::HashMap;

use glam::Vec2;
use palantir::{
    Configure, MenuItem, Panel, PopupHandle, Scroll, Sizing, Spacing, Text, TextEdit, Tooltip, Ui,
    WidgetId,
};
use scenarium::NodeId;
use scenarium::{Func, NodePorts};
use scenarium::{GraphDef, GraphId};
use scenarium::{Node, NodeKind};
use scenarium::{SPECIAL_NODES, SpecialNode};

use crate::core::document::PortRef;
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::Intent;
use crate::gui::app::AppContext;
use crate::gui::canvas::anchored_menu::AnchoredMenu;
use crate::gui::canvas::{CanvasGesture, outer_canvas_widget_id, to_world};
use crate::gui::scene::GraphScene;

/// One row of a category's palette list: a library `Func`, a built-in special
/// node, a library graph, or one of the open graph's own local definitions.
/// Collecting them into one type lets a category's rows be sorted by name
/// into a single alphabetical list.
#[derive(Debug)]
enum PaletteEntry<'a> {
    Func(&'a Func),
    Special(SpecialNode),
    Graph(GraphId, &'a GraphDef),
    LocalGraph(&'a LocalDefRow),
}

impl<'a> PaletteEntry<'a> {
    /// Borrowed from the palette's sources rather than from `self`, so a row
    /// can be grouped under its own category and moved into that group in one
    /// step.
    fn name(&self) -> &'a str {
        match *self {
            PaletteEntry::Func(f) => &f.name,
            PaletteEntry::Special(s) => &s.func().name,
            PaletteEntry::Graph(_, graph) => &graph.interface.name,
            PaletteEntry::LocalGraph(local) => &local.name,
        }
    }

    fn category(&self) -> &'a str {
        match *self {
            PaletteEntry::Func(f) => &f.category,
            PaletteEntry::Special(s) => &s.func().category,
            PaletteEntry::Graph(_, graph) => &graph.interface.category,
            PaletteEntry::LocalGraph(local) => &local.category,
        }
    }
}

/// One local definition of the graph the palette was opened over, with its
/// [`InternedStr`](palantir::InternedStr) name and category read out once
/// per open frame. The scene holds arena handles behind a `RefCell` the same
/// `Ui` interns into, so the rows own their text rather than holding borrow
/// guards across the record pass.
#[derive(Debug)]
struct LocalDefRow {
    id: GraphId,
    name: String,
    category: String,
    /// Library entry this definition was copied from, if any. The library's
    /// own row for it is dropped: with a copy already in this graph, clicking
    /// either lands on that copy (`build::local_graph_from`), so two rows
    /// would offer one outcome under one name.
    origin: Option<GraphId>,
}

/// The open graph's own local definitions, read out of the scene once per
/// frame the palette is up.
fn local_def_rows(graph: GraphScene<'_>) -> Vec<LocalDefRow> {
    graph
        .local_defs()
        .iter()
        .map(|def| LocalDefRow {
            id: def.id,
            name: def.name.borrow_str().to_string(),
            category: def.category.borrow_str().to_string(),
            origin: def.origin,
        })
        .collect()
}

/// Right-click or double-click on empty canvas → popup that lists every
/// `Func` in `AppContext::library`, plus the open graph's own local
/// definitions, grouped by category. Clicking an entry emits the intent that
/// adds it at the click's world position (inner-canvas pre-transform).
/// Outside-click and Esc dismiss.
#[derive(Default, Debug)]
pub(super) struct NewNodeUi {
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
    /// Live text of the palette's search field. Cleared on each open;
    /// case-insensitively filters the listed entries by name (a matching
    /// category name shows that whole column). Empty ⇒ everything shows.
    query: String,
}

impl NewNodeUi {
    pub(super) fn apply(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        graph: GraphScene<'_>,
        gesture: Option<CanvasGesture>,
        pending_source: Option<PortRef>,
        out: &mut Intents,
    ) {
        let resp = ui.response_for(outer_canvas_widget_id(graph.target()));
        // Open the palette either from a bare RMB / double-click (`NewNode`
        // gesture) or from a connection dropped on empty canvas
        // (`pending_source`). Placement is the same — under the pointer.
        let mut just_opened = false;
        if (pending_source.is_some() || gesture == Some(CanvasGesture::NewNode))
            && let (Some(local), Some(rect)) = (resp.pointer_local, resp.rect)
        {
            self.world_pos = to_world(local, &graph.viewport());
            self.source = pending_source;
            self.menu.open_at(rect.min + local, graph.target());
            // Fresh open: empty the filter and focus the search field this
            // frame so the user can type straight away.
            self.query.clear();
            just_opened = true;
        }

        // Cap the palette height to the window so a short window scrolls
        // the overflow (via the inner vertical `Scroll`) instead of
        // running off-screen. The popup's `max_size` height bounds the
        // whole popup; the search row sits above a `Scroll` whose own cap
        // (`max_height` minus the search row) keeps it from eating the
        // header's space — a `Hug` scroll otherwise claims the full cap.
        let surface = ui.display().logical_rect();
        let max_height = ctx
            .theme
            .new_node_popup_max_height
            .min(surface.size.h - 16.0)
            .max(120.0);
        let scroll_cap = (max_height - SEARCH_ROW_ALLOWANCE).max(80.0);
        let query = &mut self.query;
        // Rows are picked at the position the *open* captured, not wherever
        // the pointer has drifted to by the frame of the click.
        let pos = self.world_pos;
        // Inside the body, so the per-frame read only happens on the frames
        // the palette is actually up — `show` skips the body when closed.
        let chosen = self.menu.show(
            ui,
            graph.target(),
            "new_node_popup",
            Some(max_height),
            |ui, popup| {
                let palette = Palette {
                    ctx,
                    pos,
                    local_defs: local_def_rows(graph),
                };
                palette_body(ui, popup, &palette, query, scroll_cap, just_opened)
            },
        );

        if let Some(intent) = chosen {
            out.for_graph(graph.target(), |out| out.push(intent));
            // If a dropped connection opened this popup, hand its source
            // back so the wire resumes floating — the user then clicks the
            // exact port to land it, rather than it auto-attaching.
            self.resume_floating = self.source;
        }
    }

    /// Take the source of a wire whose drop spawned a node this frame — the
    /// canvas re-floats it on `ConnectionUI`. `None` on a plain palette open.
    pub(super) fn take_resume_floating(&mut self) -> Option<PortRef> {
        self.resume_floating.take()
    }
}

/// Gap (px) below the search field, before the results scroll.
const SEARCH_ROW_GAP: f32 = 8.0;

/// Vertical space (px) the search row (field + its [`SEARCH_ROW_GAP`]) and
/// popup padding claim above the scrolling results, subtracted from the
/// popup's height cap to size the inner `Scroll`.
const SEARCH_ROW_ALLOWANCE: f32 = 48.0;

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
    query: &mut String,
    scroll_cap: f32,
    focus: bool,
) -> Option<Intent> {
    let mut chosen: Option<Intent> = None;

    let search_id = WidgetId::from_hash("new_node_search");
    TextEdit::new(query)
        .id(search_id)
        .placeholder("Search…")
        .style(&palette.ctx.theme.inline_rename.text_edit)
        .size((Sizing::fill(1.0), Sizing::HUG))
        .min_size((200.0, 0.0))
        .margin(Spacing::new(0.0, 0.0, 0.0, SEARCH_ROW_GAP))
        .show(ui);
    if focus {
        ui.request_focus(Some(search_id));
    }
    let query_lc = query.to_lowercase();

    Scroll::vertical()
        .id_salt("new_node_scroll")
        .size((Sizing::HUG, Sizing::HUG))
        .max_size((f32::INFINITY, scroll_cap))
        .show(ui, |ui| {
            Panel::hstack()
                .id_salt("new_node_columns")
                .size((Sizing::HUG, Sizing::HUG))
                .gap(12.0)
                .show(ui, |ui| {
                    for column in palette.columns(&query_lc) {
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
    ctx: &'a AppContext<'a>,
    /// World position the open captured — every intent a row raises places
    /// its node here.
    pos: Vec2,
    local_defs: Vec<LocalDefRow>,
}

/// One category's rows, ready to record.
#[derive(Debug)]
struct PaletteColumn<'a> {
    category: &'a str,
    entries: Vec<PaletteEntry<'a>>,
}

impl<'a> Palette<'a> {
    /// Every row the palette can list, in no particular order. The library's
    /// row for a graph this document already copied is left out — see
    /// [`LocalDefRow::origin`].
    fn entries(&'a self) -> impl Iterator<Item = PaletteEntry<'a>> {
        self.ctx
            .library
            .funcs()
            .map(PaletteEntry::Func)
            .chain(SPECIAL_NODES.iter().copied().map(PaletteEntry::Special))
            .chain(
                self.ctx
                    .library
                    .graphs
                    .iter()
                    .filter(|(id, _)| {
                        !self
                            .local_defs
                            .iter()
                            .any(|local| local.origin.as_ref() == Some(id))
                    })
                    .map(|(id, graph)| PaletteEntry::Graph(*id, graph)),
            )
            .chain(self.local_defs.iter().map(PaletteEntry::LocalGraph))
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
        columns.sort_by_key(|column| column.category);
        columns
    }
}

impl PaletteColumn<'_> {
    /// Record this column: its category name above its rows.
    fn show(self, ui: &mut Ui, popup: &PopupHandle, palette: &Palette<'_>) -> Option<Intent> {
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
/// for a graph the user named in their own script.
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
    /// here — `build_step` seeds them off the definition it resolves.
    fn show(self, ui: &mut Ui, popup: &PopupHandle, palette: &Palette<'_>) -> Option<Intent> {
        let pos = palette.pos;
        match self {
            PaletteEntry::Func(func) => menu_row(ui, popup, func).then(|| {
                let node_id = NodeId::unique();
                Intent::AddNode {
                    pos,
                    node_id,
                    node: func.into(),
                    bindings: NodePorts::from(func).default_bindings(node_id).collect(),
                }
            }),
            // A special node's `Func` is hardcoded rather than
            // library-registered, so the node it spawns is a
            // `NodeKind::Special` named after that func.
            PaletteEntry::Special(special) => {
                let func = special.func();
                menu_row(ui, popup, func).then(|| {
                    let node_id = NodeId::unique();
                    let mut node = Node::new(NodeKind::Special(special));
                    node.name = func.name.clone();
                    Intent::AddNode {
                        pos,
                        node_id,
                        node,
                        bindings: NodePorts::from(func).default_bindings(node_id).collect(),
                    }
                })
            }
            // A library graph localizes on instance: the copy records its
            // `origin` so it stays linked for a later publish, but it is the
            // document's to edit from here on.
            PaletteEntry::Graph(shared_id, graph) => menu_item(ui, popup, &graph.interface.name)
                .then(|| {
                    let mut local = graph.clone_mapped();
                    local.interface.origin = Some(shared_id);
                    Intent::AddLocalGraph {
                        pos,
                        node_id: NodeId::unique(),
                        graph_id: GraphId::unique(),
                        def: Box::new(local),
                    }
                }),
            // No copy: a second instance of a definition this graph already
            // holds, so editing either instance's interior edits the one
            // definition.
            PaletteEntry::LocalGraph(local) => {
                menu_item(ui, popup, &local.name).then(|| Intent::AddLocalGraphInstance {
                    pos,
                    node_id: NodeId::unique(),
                    graph_id: local.id,
                })
            }
        }
    }
}

/// Record one plain palette row, reporting whether it was clicked.
fn menu_item(ui: &mut Ui, popup: &PopupHandle, name: &str) -> bool {
    MenuItem::new(name).show(ui, popup).left.clicked()
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
mod tests {
    use super::*;
    use crate::gui::run_state::RunState;
    use crate::gui::theme::Theme;

    #[test]
    fn name_matches_is_case_insensitive_substring_with_empty_query_wildcard() {
        // Empty query is the "show everything" wildcard.
        assert!(name_matches("Gaussian Blur", ""));
        assert!(name_matches("", ""));
        // Case-insensitive substring anywhere in the name. Caller passes an
        // already-lowercased query, so only the name is folded here.
        assert!(name_matches("Gaussian Blur", "blur"));
        assert!(name_matches("Gaussian Blur", "gauss"));
        assert!(name_matches("Gaussian Blur", "an bl"));
        // Non-substring and wrong-fragment queries reject.
        assert!(!name_matches("Gaussian Blur", "sharpen"));
        assert!(!name_matches("Blur", "blurry"));
        // A non-lowercased query never matches a lowercased name — the
        // contract is "query already lowercased", so this documents that a
        // caller who forgets to fold gets no false positives. It holds on
        // both sides of the ASCII fast path.
        assert!(!name_matches("blur", "BLUR"));
        assert!(!name_matches("Grün", "GRÜN"));
        // Non-ASCII names fold by the Unicode rules, not byte-wise.
        assert!(name_matches("Grün", "grün"));
        assert!(name_matches("Ölfilter", "ölfil"));
        assert!(!name_matches("Grün", "grun"));
        // An ASCII name never matches a non-ASCII query.
        assert!(!name_matches("Blur", "blür"));
    }

    /// A local-definition row built without a scene, for the listing tests.
    /// The scene's own projection into these rows is covered by
    /// `gui::scene::tests::local_defs_project_per_pane_ordered_by_id`.
    fn local_row(name: &str, category: &str) -> LocalDefRow {
        LocalDefRow {
            id: GraphId::unique(),
            name: name.to_owned(),
            category: category.to_owned(),
            origin: None,
        }
    }

    /// The rows of the column named `category`, or `None` when the query
    /// leaves that category with no column at all.
    fn column<'a>(columns: &'a [PaletteColumn<'a>], category: &str) -> Option<Vec<&'a str>> {
        let found = columns.iter().find(|c| c.category == category)?;
        Some(found.entries.iter().map(PaletteEntry::name).collect())
    }

    #[test]
    fn the_open_graphs_own_definitions_list_beside_the_librarys() {
        use scenarium::{GraphDef, Library};

        let origin = GraphId::unique();
        let mut library = Library::default();
        library.register_graph(origin, GraphDef::new("Published").category("Document"));
        library.register_graph(
            GraphId::unique(),
            GraphDef::new("Untouched").category("Document"),
        );
        let theme = Theme::default();
        let run_state = RunState::default();
        let ctx = AppContext {
            theme: &theme,
            library: &library,
            run_state: &run_state,
            status_error: None,
        };
        let palette = |local_defs| Palette {
            ctx: &ctx,
            pos: Vec2::ZERO,
            local_defs,
        };

        // Local definitions land in the column their own category names,
        // interleaved with the library's rows in one alphabetical list, and
        // a category only they use still gets a column of its own.
        let listed = palette(vec![
            local_row("Sharpen", "Document"),
            local_row("Blur", "Document"),
            local_row("Offstage", "Elsewhere"),
        ]);
        let columns = listed.columns("");
        assert_eq!(
            column(&columns, "Document").as_deref(),
            Some(["Blur", "Published", "Sharpen", "Untouched"].as_slice())
        );
        assert_eq!(
            column(&columns, "Elsewhere").as_deref(),
            Some(["Offstage"].as_slice())
        );
        assert_eq!(
            columns.iter().filter(|c| c.category == "Document").count(),
            1,
            "one column per category, deduped across the four sources"
        );
        // Columns are ordered by category name, so panes don't reshuffle.
        let mut sorted = columns.iter().map(|c| c.category).collect::<Vec<_>>();
        sorted.sort();
        assert_eq!(
            columns.iter().map(|c| c.category).collect::<Vec<_>>(),
            sorted
        );

        // The query filters local rows by name like any other entry; a
        // matching category name reveals that whole column; a category left
        // with nothing gets no column at all.
        assert_eq!(
            column(&listed.columns("sharp"), "Document").as_deref(),
            Some(["Sharpen"].as_slice())
        );
        assert_eq!(
            column(&listed.columns("docum"), "Document").map(|rows| rows.len()),
            Some(4),
            "a category-name match shows every row under it"
        );
        assert_eq!(
            column(&listed.columns("nothing matches this"), "Document"),
            None,
            "a category with no matching row renders no column"
        );

        // With a local copy of the library entry already in this graph, the
        // library's own row for it goes: clicking either lands on the copy.
        let mut copy = local_row("Published", "Document");
        copy.origin = Some(origin);
        assert_eq!(
            column(&palette(vec![copy]).columns(""), "Document").as_deref(),
            Some(["Published", "Untouched"].as_slice()),
            "one row per definition, not two under the same name"
        );
    }
}
