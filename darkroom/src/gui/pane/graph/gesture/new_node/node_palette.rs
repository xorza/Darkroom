//! [`NodePalette`]: everything one new-node popup lists, and the rows and
//! columns it records.

use std::cmp::Ordering;

use glam::Vec2;
use palantir::{
    CloseHandle, Configure, MenuItem, Panel, Scroll, Sizing, Spacing, Text, TextEdit, Tooltip, Ui,
};
use scenarium::Func;
use scenarium::NodeId;
use scenarium::{Node, NodeKind};
use scenarium::{SPECIAL_NODES, SpecialNode};

use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::gesture::new_node::{
    SEARCH_ROW_GAP, Search, results_wid, search_field_wid,
};

/// One row of a category's palette list: a library `Func` or a built-in
/// special node. Collecting them into one type lets every source sort
/// into one list, which is what makes a category a run inside it.
#[derive(Clone, Copy, Debug)]
enum PaletteEntry<'a> {
    Func(&'a Func),
    Special(SpecialNode),
}

impl<'a> PaletteEntry<'a> {
    /// Borrowed from the palette's sources rather than from `self`, so a
    /// name outlives any borrow of the row that yielded it.
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

    /// The rows `query_lc` matches, category-major then name — so a
    /// category's rows are the contiguous run [`PaletteColumn::runs`] reads.
    ///
    /// One `Vec` rather than a map of per-category ones: the palette rebuilds
    /// this every frame it is up, and grouping through a map spent the map's
    /// allocation plus one per category on each of those frames. Sorting by
    /// the category first puts the same rows in the same groups for a single
    /// allocation, taken from `entries`' own upper bound so the fill never
    /// grows it.
    ///
    /// A matching *category* name reveals that whole column; otherwise a row
    /// is filtered by its own name. Asked per row, which is what drops the
    /// grouping pass the map needed before it could ask per category.
    fn matching(entries: impl Iterator<Item = Self>, query_lc: &str) -> Vec<Self> {
        let mut rows: Vec<Self> = Vec::with_capacity(entries.size_hint().1.unwrap_or_default());
        rows.extend(entries.filter(|entry| {
            name_matches(entry.category(), query_lc) || name_matches(entry.name(), query_lc)
        }));
        // Case-insensitive by comparison, not by key: `sort_by_cached_key` with
        // a `to_lowercase()` key allocated one `String` per row per frame to
        // answer a question `char`-wise folding answers in place. The raw-order
        // fallback keeps two categories that fold alike in runs of their own.
        rows.sort_by(|a, b| {
            lowercase_cmp(a.category(), b.category())
                .then_with(|| lowercase_cmp(a.name(), b.name()))
        });
        rows
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

/// One category's rows, ready to record: a run inside the buffer
/// [`PaletteEntry::matching`] sorted, not a list of its own.
#[derive(Debug)]
struct PaletteColumn<'a> {
    category: &'a str,
    entries: &'a [PaletteEntry<'a>],
}

impl<'a> NodePalette<'a> {
    pub(super) fn new(graph_ctx: GraphCtx<'a>, pos: Vec2) -> Self {
        Self { graph_ctx, pos }
    }

    /// Every row the palette can list, in no particular order: the library's
    /// funcs, then the built-in specials.
    fn entries(&'a self) -> impl Iterator<Item = PaletteEntry<'a>> {
        self.graph_ctx
            .library()
            .funcs()
            .map(PaletteEntry::Func)
            .chain(SPECIAL_NODES.iter().copied().map(PaletteEntry::Special))
    }
}

impl<'a> PaletteColumn<'a> {
    /// The category runs of `rows`, in order. Requires the category-major
    /// ordering [`PaletteEntry::matching`] leaves — an unsorted buffer
    /// splits one category into as many columns as it has stretches.
    fn runs(rows: &'a [PaletteEntry<'a>]) -> impl Iterator<Item = Self> {
        rows.chunk_by(|a, b| a.category() == b.category())
            .map(|entries| Self {
                category: entries[0].category(),
                entries,
            })
    }

    /// Record this column: its category name above its rows.
    fn show(
        self,
        ui: &mut Ui,
        popup: &CloseHandle,
        palette: &NodePalette<'_>,
    ) -> Option<GraphIntent> {
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
                        for &entry in self.entries {
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
    pub(super) fn body(
        &self,
        ui: &mut Ui,
        popup: &CloseHandle,
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
            .style(&self.graph_ctx.theme().inline_rename.text_edit)
            .size((Sizing::fill(1.0), Sizing::HUG))
            .min_size((200.0, 0.0))
            .margin(Spacing::new(0.0, 0.0, 0.0, SEARCH_ROW_GAP))
            .show(ui);
        if focus {
            ui.request_focus(Some(search_id));
        }
        // Folded after the field records, so it reflects this frame's typing.
        search.fold();
        let rows = PaletteEntry::matching(self.entries(), &search.folded);

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
                        for column in PaletteColumn::runs(&rows) {
                            if let Some(picked) = column.show(ui, popup, self) {
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

#[cfg(test)]
mod tests {
    use scenarium::{Func, FuncId};

    use crate::gui::pane::graph::gesture::new_node::node_palette::{
        PaletteColumn, PaletteEntry, name_matches,
    };

    /// Four rows over three categories, deliberately out of order and mixing
    /// case so the fold and the raw-order fallback both have to fire.
    fn funcs() -> Vec<Func> {
        ["Zoom/crop", "blur/Sharpen", "Blur/gaussian", "blur/box"]
            .into_iter()
            .map(|spec| {
                let (category, name) = spec.split_once('/').unwrap();
                Func::new(FuncId::unique(), name).category(category)
            })
            .collect()
    }

    fn rows<'a>(funcs: &'a [Func], query_lc: &str) -> Vec<PaletteEntry<'a>> {
        PaletteEntry::matching(funcs.iter().map(PaletteEntry::Func), query_lc)
    }

    fn shape<'a>(rows: &'a [PaletteEntry<'a>]) -> Vec<(&'a str, Vec<&'a str>)> {
        PaletteColumn::runs(rows)
            .map(|column| {
                (
                    column.category,
                    column.entries.iter().map(PaletteEntry::name).collect(),
                )
            })
            .collect()
    }

    /// The palette groups by sorting, so the buffer's order *is* the column
    /// layout: categories folded-alphabetically, rows the same inside each,
    /// and every category one contiguous run.
    #[test]
    fn rows_sort_into_one_contiguous_run_per_category() {
        let funcs = funcs();

        // "Blur" and "blur" fold alike, so they sort adjacently — and stay
        // two runs, because the fallback orders them by the raw name.
        assert_eq!(
            shape(&rows(&funcs, "")),
            [
                ("Blur", vec!["gaussian"]),
                ("blur", vec!["box", "Sharpen"]),
                ("Zoom", vec!["crop"]),
            ],
            "no query lists every row, category-major then name",
        );

        // A query the *category* carries reveals both blur columns whole,
        // including the row whose own name holds no "blur".
        assert_eq!(
            shape(&rows(&funcs, "blur")),
            [("Blur", vec!["gaussian"]), ("blur", vec!["box", "Sharpen"])],
            "a category match keeps its rows whatever they are named",
        );

        // A query only a row name carries takes that row and drops the rest
        // of its category with it.
        assert_eq!(
            shape(&rows(&funcs, "box")),
            [("blur", vec!["box"])],
            "a name match keeps the row alone",
        );

        assert!(
            shape(&rows(&funcs, "nothing")).is_empty(),
            "a query nothing carries lists no column at all",
        );
    }

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
}
