use std::collections::HashSet;

use glam::Vec2;
use palantir::{
    Configure, MenuItem, Panel, PopupHandle, Scroll, Sizing, Spacing, Text, TextEdit, Tooltip, Ui,
    WidgetId,
};
use scenarium::NodeId;
use scenarium::{Binding, InputPort, Node, NodeKind};
use scenarium::{Func, NodePorts};
use scenarium::{GraphDef, GraphId, GraphLink};
use scenarium::{SPECIAL_NODES, SpecialNode};

use crate::core::document::PortRef;
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::Intent;
use crate::gui::app::AppContext;
use crate::gui::canvas::anchored_menu::AnchoredMenu;
use crate::gui::canvas::{CanvasGesture, outer_canvas_widget_id, to_world};
use crate::gui::scene::GraphScene;

/// A chosen palette entry, as the intent it raises.
///
/// A local definition is picked by id alone: it is already in the graph, so
/// unlike every other row there is no node, definition, or binding for the
/// palette to build — `build_step` reads them off the definition the id
/// names.
#[derive(Debug)]
enum ChosenNode {
    /// A func, special node, or library graph: the node and the state
    /// created atomically with it.
    New {
        node_id: NodeId,
        node: Node,
        graph: Option<(GraphId, Box<GraphDef>)>,
        bindings: Vec<(InputPort, Binding)>,
    },
    /// One of the open graph's own local definitions.
    LocalInstance { node_id: NodeId, graph_id: GraphId },
}

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

impl PaletteEntry<'_> {
    fn name(&self) -> &str {
        match self {
            PaletteEntry::Func(f) => &f.name,
            PaletteEntry::Special(s) => &s.func().name,
            PaletteEntry::Graph(_, graph) => &graph.interface.name,
            PaletteEntry::LocalGraph(local) => &local.name,
        }
    }
}

/// One local definition of the graph the palette was opened over, with its
/// [`InternedStr`](palantir::InternedStr) name and category read out once
/// per open frame. The scene holds arena handles, and filtering, sorting,
/// and matching all want `&str` — copying here keeps that to one pass
/// instead of one per category column.
#[derive(Debug)]
struct LocalDefRow {
    id: GraphId,
    name: String,
    category: String,
}

/// The open graph's own local definitions, and the library ids they were
/// copied from.
///
/// The library rows for those origins are dropped: with a local copy already
/// in this graph, clicking the library row lands on that same copy
/// (`build::reuse_local_graph`), so the two rows are one outcome under one
/// name.
#[derive(Debug, Default)]
struct LocalDefs {
    rows: Vec<LocalDefRow>,
    covered_origins: HashSet<GraphId>,
}

impl LocalDefs {
    fn read(graph: GraphScene<'_>) -> Self {
        let defs = graph.local_defs();
        let mut rows = Vec::with_capacity(defs.len());
        let mut covered_origins = HashSet::new();
        for def in defs {
            rows.push(LocalDefRow {
                id: def.id,
                name: def.name.borrow_str().to_string(),
                category: def.category.borrow_str().to_string(),
            });
            covered_origins.extend(def.origin);
        }
        Self {
            rows,
            covered_origins,
        }
    }
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
        let local_defs = LocalDefs::read(graph);
        let chosen = self.menu.show(
            ui,
            graph.target(),
            "new_node_popup",
            Some(max_height),
            |ui, popup| palette_body(ui, popup, ctx, &local_defs, query, scroll_cap, just_opened),
        );

        if let Some(chosen) = chosen {
            let pos = self.world_pos;
            out.for_graph(graph.target(), |out| {
                out.push(match chosen {
                    ChosenNode::New {
                        node_id,
                        node,
                        graph: definition,
                        bindings,
                    } => Intent::AddNode {
                        pos,
                        node_id,
                        node,
                        graph: definition,
                        bindings,
                    },
                    ChosenNode::LocalInstance { node_id, graph_id } => {
                        Intent::AddLocalGraphInstance {
                            pos,
                            node_id,
                            graph_id,
                        }
                    }
                })
            });
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
    ctx: &AppContext<'_>,
    local_defs: &LocalDefs,
    query: &mut String,
    scroll_cap: f32,
    focus: bool,
) -> Option<ChosenNode> {
    let mut chosen: Option<ChosenNode> = None;

    let search_id = WidgetId::from_hash("new_node_search");
    TextEdit::new(query)
        .id(search_id)
        .placeholder("Search…")
        .style(&ctx.theme.inline_rename.text_edit)
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
                    for category in sorted_categories(ctx, local_defs) {
                        if let Some(picked) =
                            category_column(ui, popup, ctx, local_defs, category, &query_lc)
                        {
                            chosen = Some(picked);
                        }
                    }
                });
        });
    chosen
}

/// Everything one category lists, in display order: funcs, built-in special
/// nodes, library graphs, and the open graph's own local definitions,
/// filtered by the query, merged, and sorted by name into a single
/// alphabetical list. Empty when nothing in the category matches, which is
/// what makes the column render nothing at all.
///
/// A matching *category* name reveals the whole column; otherwise each entry
/// is filtered by its own name.
fn category_entries<'a>(
    ctx: &'a AppContext<'_>,
    local_defs: &'a LocalDefs,
    category: &str,
    query_lc: &str,
) -> Vec<PaletteEntry<'a>> {
    let cat_match = name_matches(category, query_lc);
    let shows = |name: &str| cat_match || name_matches(name, query_lc);
    let mut entries: Vec<PaletteEntry> = ctx
        .library
        .funcs()
        .filter(|f| f.category == category && shows(&f.name))
        .map(PaletteEntry::Func)
        .chain(
            SPECIAL_NODES
                .iter()
                .copied()
                .filter(|s| s.func().category == category && shows(&s.func().name))
                .map(PaletteEntry::Special),
        )
        .chain(
            ctx.library
                .graphs
                .iter()
                .filter(|(id, graph)| {
                    let interface = &graph.interface;
                    !local_defs.covered_origins.contains(*id)
                        && interface.category == category
                        && shows(&interface.name)
                })
                .map(|(id, graph)| PaletteEntry::Graph(*id, graph)),
        )
        .chain(
            local_defs
                .rows
                .iter()
                .filter(|local| local.category == category && shows(&local.name))
                .map(PaletteEntry::LocalGraph),
        )
        .collect();
    entries.sort_by_cached_key(|e| e.name().to_ascii_lowercase());
    entries
}

/// One category's palette column: the category name above
/// [`category_entries`]. Returns `None` (and renders nothing) when the
/// category has no matching entry.
fn category_column(
    ui: &mut Ui,
    popup: &PopupHandle,
    ctx: &AppContext<'_>,
    local_defs: &LocalDefs,
    category: &str,
    query_lc: &str,
) -> Option<ChosenNode> {
    let entries = category_entries(ctx, local_defs, category, query_lc);
    if entries.is_empty() {
        return None;
    }

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
                    for entry in entries {
                        let picked = match entry {
                            PaletteEntry::Func(func) => func_entry(ui, popup, func),
                            PaletteEntry::Special(special) => special_entry(ui, popup, special),
                            PaletteEntry::Graph(id, graph) => graph_entry(ui, popup, id, graph),
                            PaletteEntry::LocalGraph(local) => local_graph_entry(ui, popup, local),
                        };
                        if let Some(picked) = picked {
                            chosen = Some(picked);
                        }
                    }
                });
        });
    chosen
}

/// Case-insensitive substring match used by the palette search. An empty
/// (already-lowercased) query matches everything.
fn name_matches(name: &str, query_lc: &str) -> bool {
    query_lc.is_empty() || name.to_lowercase().contains(query_lc)
}

/// Every category that has a func, a built-in special node, a library graph,
/// or one of the open graph's local definitions, sorted + deduped — one popup
/// column each.
fn sorted_categories<'a>(ctx: &'a AppContext<'_>, local_defs: &'a LocalDefs) -> Vec<&'a str> {
    let mut cats: Vec<&str> = ctx
        .library
        .funcs()
        .map(|f| f.category.as_str())
        .chain(
            ctx.library
                .graphs
                .values()
                .map(|graph| graph.interface.category.as_str()),
        )
        .chain(SPECIAL_NODES.iter().map(|s| s.func().category.as_str()))
        .chain(local_defs.rows.iter().map(|local| local.category.as_str()))
        .collect();
    cats.sort();
    cats.dedup();
    cats
}

/// One palette row for a library `Func`: on click, the node it spawns plus its
/// declared input defaults. Hovering shows the func's description.
fn func_entry(ui: &mut Ui, popup: &PopupHandle, func: &Func) -> Option<ChosenNode> {
    let resp = MenuItem::new(&func.name).show(ui, popup);
    let clicked = resp.left.clicked();
    if let Some(desc) = &func.description {
        Tooltip::on(&resp.snapshot()).text(desc).show(ui);
    }
    clicked.then(|| {
        let node_id = NodeId::unique();
        let node: Node = func.into();
        let bindings = NodePorts::from(func).default_bindings(node_id).collect();
        ChosenNode::New {
            node_id,
            node,
            graph: None,
            bindings,
        }
    })
}

/// One palette row for a shared graph: on click, an instance node plus an
/// editable `Local` copy of the graph that records its library `origin`, so the
/// instance localizes rather than staying linked to the library entry.
fn graph_entry(
    ui: &mut Ui,
    popup: &PopupHandle,
    shared_id: GraphId,
    graph: &GraphDef,
) -> Option<ChosenNode> {
    let interface = &graph.interface;
    if !MenuItem::new(&interface.name)
        .show(ui, popup)
        .left
        .clicked()
    {
        return None;
    }
    let local_id = GraphId::unique();
    let mut local = graph.clone_mapped();
    local.interface.origin = Some(shared_id);
    let node_id = NodeId::unique();
    let node = Node::graph_instance(&local, GraphLink::Local(local_id));
    let bindings = local.ports().default_bindings(node_id).collect();
    Some(ChosenNode::New {
        node_id,
        node,
        graph: Some((local_id, Box::new(local))),
        bindings,
    })
}

/// One palette row for a local definition the open graph already holds: on
/// click, a second instance node pointing at that same definition. No copy is
/// made — editing either instance's interior edits the one definition.
fn local_graph_entry(ui: &mut Ui, popup: &PopupHandle, local: &LocalDefRow) -> Option<ChosenNode> {
    MenuItem::new(&local.name)
        .show(ui, popup)
        .left
        .clicked()
        .then(|| ChosenNode::LocalInstance {
            node_id: NodeId::unique(),
            graph_id: local.id,
        })
}

/// One palette entry for a built-in special node. Its `Func` (interface) is
/// hardcoded, not library-registered, so the spawned `Node` is a
/// `NodeKind::Special` named after that func, with its declared input defaults
/// seeded like a func node.
fn special_entry(ui: &mut Ui, popup: &PopupHandle, special: SpecialNode) -> Option<ChosenNode> {
    let func = special.func();
    let resp = MenuItem::new(&func.name).show(ui, popup);
    let clicked = resp.left.clicked();
    if let Some(desc) = &func.description {
        Tooltip::on(&resp.snapshot()).text(desc).show(ui);
    }
    if !clicked {
        return None;
    }
    let node_id = NodeId::unique();
    let mut node = Node::new(NodeKind::Special(special));
    node.name = func.name.clone();
    let bindings = NodePorts::from(func).default_bindings(node_id).collect();
    Some(ChosenNode::New {
        node_id,
        node,
        graph: None,
        bindings,
    })
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
        // caller who forgets to fold gets no false positives.
        assert!(!name_matches("blur", "BLUR"));
    }

    /// A [`LocalDefs`] built without a scene, for the listing tests. The
    /// scene's own projection into these rows is covered by
    /// `gui::scene::tests::local_defs_project_per_pane_ordered_by_id`.
    fn local_defs(rows: Vec<LocalDefRow>) -> LocalDefs {
        LocalDefs {
            covered_origins: HashSet::new(),
            rows,
        }
    }

    fn local_row(name: &str, category: &str) -> LocalDefRow {
        LocalDefRow {
            id: GraphId::unique(),
            name: name.to_owned(),
            category: category.to_owned(),
        }
    }

    fn names<'a>(entries: &'a [PaletteEntry<'a>]) -> Vec<&'a str> {
        entries.iter().map(PaletteEntry::name).collect()
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

        // Local definitions land in the column their own category names,
        // interleaved with the library's rows in one alphabetical list.
        let defs = local_defs(vec![
            local_row("Sharpen", "Document"),
            local_row("Blur", "Document"),
            local_row("Offstage", "Elsewhere"),
        ]);
        assert_eq!(
            names(&category_entries(&ctx, &defs, "Document", "")),
            ["Blur", "Published", "Sharpen", "Untouched"]
        );
        assert_eq!(
            names(&category_entries(&ctx, &defs, "Elsewhere", "")),
            ["Offstage"]
        );
        assert!(
            category_entries(&ctx, &defs, "Empty", "").is_empty(),
            "a category with no entry renders no column"
        );

        // The query filters local rows by name like any other entry, and a
        // matching category name still reveals the whole column.
        assert_eq!(
            names(&category_entries(&ctx, &defs, "Document", "sharp")),
            ["Sharpen"]
        );
        assert_eq!(
            names(&category_entries(&ctx, &defs, "Document", "docum")).len(),
            4,
            "a category-name match shows every row under it"
        );

        // With a local copy of the library entry already in this graph, the
        // library's own row for it goes: clicking either lands on the copy.
        let mut copied = local_defs(vec![local_row("Published", "Document")]);
        copied.covered_origins.insert(origin);
        assert_eq!(
            names(&category_entries(&ctx, &copied, "Document", "")),
            ["Published", "Untouched"],
            "one row per definition, not two under the same name"
        );

        // Categories come from the local definitions too, or a definition in
        // a category nothing else uses would have no column to sit in.
        let cats = sorted_categories(&ctx, &defs);
        assert!(cats.contains(&"Document") && cats.contains(&"Elsewhere"));
        assert_eq!(
            cats.iter().filter(|c| **c == "Document").count(),
            1,
            "categories are deduped across the four sources"
        );
    }
}
