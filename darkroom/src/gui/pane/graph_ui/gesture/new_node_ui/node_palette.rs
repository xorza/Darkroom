//! [`NodePalette`]: everything one new-node popup lists, and the rows and
//! columns it records.

use std::ops::Range;

use palantir::prelude::*;
use palantir::{CloseHandle, FontWeight, KeyboardWake, TextWrap};
use scenarium::Func;
use scenarium::Node;
use scenarium::NodeId;
use scenarium::{SPECIAL_NODES, SpecialNode};

use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::gesture::new_node_ui;
use crate::gui::pane::graph_ui::gesture::new_node_ui::palette_rows::PaletteRow;
use crate::gui::pane::graph_ui::gesture::new_node_ui::{SEARCH_ROW_GAP, Search};
use crate::gui::theme::Theme;
use crate::gui::widgets::support;

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
            PaletteEntry::Func(f) => f.name(),
            PaletteEntry::Special(s) => s.func().name(),
        }
    }

    pub(super) fn category(self) -> &'a str {
        match self {
            PaletteEntry::Func(f) => f.category(),
            PaletteEntry::Special(s) => s.func().category(),
        }
    }

    /// The first line of what the function does, if it says.
    pub(super) fn description(self) -> Option<&'a str> {
        self.func()
            .description()
            .map(|text| text.lines().next().unwrap_or(text))
    }

    /// The declaration the row lists and its node is named after.
    fn func(self) -> &'a Func {
        match self {
            PaletteEntry::Func(f) => f,
            PaletteEntry::Special(s) => s.func(),
        }
    }

    /// The node a pick of this row adds. A special node's `Func` is hardcoded
    /// rather than library-registered, so the node it spawns comes from the
    /// [`SpecialNode`] itself, which reads the same
    /// hardcoded func for its name and cache mode.
    fn node(self) -> Node {
        match self {
            PaletteEntry::Func(f) => f.into(),
            PaletteEntry::Special(s) => s.into(),
        }
    }

    /// The identity
    /// [`PaletteRows`](crate::gui::pane::graph_ui::gesture::new_node_ui::palette_rows::PaletteRows)
    /// keeps for this entry.
    pub(super) const fn row(self) -> PaletteRow {
        match self {
            PaletteEntry::Func(f) => PaletteRow::Func(f.id()),
            PaletteEntry::Special(s) => PaletteRow::Special(s),
        }
    }
}

/// Everything one palette open lists, plus where a pick lands and how the
/// frame lays it out. Built once per body from state the whole body shares,
/// so the column and row helpers take one borrow rather than four threaded
/// parameters.
#[derive(Debug)]
pub(super) struct NodePalette<'a> {
    pub(super) graph_ctx: GraphCtx<'a>,
    /// World position the open captured — every intent a row raises places
    /// its node here.
    pub(super) pos: Vec2,
    /// The most height the results may scroll in.
    pub(super) scroll_cap: f32,
    pub(super) opening: Opening,
}

/// Whether the palette opened this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Opening {
    /// Opened this frame: the search field takes the focus, and the rows are
    /// filtered afresh against the library as it is now.
    Fresh,
    /// Open since an earlier frame.
    Standing,
}

impl<'a> NodePalette<'a> {
    /// Every row the palette can list, in no particular order: the library's
    /// funcs, then the built-in specials.
    fn entries(&self) -> impl Iterator<Item = PaletteEntry<'a>> {
        self.graph_ctx
            .library()
            .iter()
            .map(PaletteEntry::Func)
            .chain(SPECIAL_NODES.iter().copied().map(PaletteEntry::Special))
    }
}

impl NodePalette<'_> {
    /// Record the search field and the rows `search` matches, and move or pick the focused row
    /// on Up, Down and Enter.
    pub(super) fn body(
        &self,
        ui: &mut Ui,
        popup: &CloseHandle,
        search: &mut Search,
    ) -> Option<GraphIntent> {
        let opened = self.opening == Opening::Fresh;
        let theme = self.graph_ctx.theme();
        let search_id = new_node_ui::search_field_wid();
        let library = self.graph_ctx.library();
        // The search row: the field on the rows' left edge, and what the keys do at the right.
        let mut submitted = false;
        let mut reveal = opened;
        Panel::hstack()
            .id(new_node_ui::search_row_wid())
            .size((Sizing::fill(1.0), Sizing::HUG))
            .min_size((LIST_WIDTH, 0.0))
            .padding(Spacing::xy(theme.space.sm, theme.space.xs))
            .gap(theme.space.md)
            .child_align(Align::v(VAlign::Center))
            .show(ui, |ui| {
                let field = TextEdit::new(&mut search.text)
                    .id(search_id)
                    // The field filters the palette rather than editing a value, so Esc
                    // closes the whole popup instead of blurring the only thing in it
                    // the user can type into.
                    .escape_falls_through(true)
                    .placeholder("Search…")
                    .style(&theme.inline_rename.text_edit)
                    .size((Sizing::fill(1.0), Sizing::HUG))
                    .show(ui);
                submitted = field.ended == Some(EditEnd::Submitted);
                if opened {
                    ui.set_focus(search_id);
                }
                // Folded after the field records, so it reflects this frame's typing.
                if search.fold() || opened {
                    search
                        .rows
                        .refilter(library, self.entries(), &search.folded);
                    search.focused = 0;
                    reveal = true;
                }
                let hint = if search.rows.len() == 0 {
                    "esc close"
                } else {
                    "\u{21b5} add \u{b7} \u{2191}\u{2193} move"
                };
                let style = support::muted_text(ui, theme, theme.text.label);
                Text::new(hint).style(&style).show(ui);
            });
        Separator::horizontal()
            .margin(Spacing::new(0.0, 0.0, 0.0, SEARCH_ROW_GAP))
            .show(ui);
        // Read off the popup's own presses rather than `key_pressed`: the search field holds the
        // focus and its scope takes the arrow keys, which a single-line field leaves unused.
        ui.watch_keyboard(KeyboardWake::KEY);
        let last = search.rows.len().saturating_sub(1);
        for press in ui.keyboard_events() {
            let to = match press.key {
                Key::ArrowDown => (search.focused + 1).min(last),
                Key::ArrowUp => search.focused.saturating_sub(1),
                _ => continue,
            };
            reveal |= to != search.focused;
            search.focused = to;
        }
        let mut chosen = None;
        let mut results = Scroll::vertical()
            .id(new_node_ui::results_wid())
            .size((Sizing::HUG, Sizing::HUG))
            .max_size((f32::INFINITY, self.scroll_cap));
        // Only when the keys or the query moved the focus: a reveal every frame would pull the
        // list back from a wheel that scrolled the focused row away.
        if reveal {
            results = results.reveal(row_wid(search.focused));
        }
        results.show(ui, |ui| {
            Panel::vstack()
                .id_salt("new_node_rows")
                .size((Sizing::fixed(LIST_WIDTH), Sizing::HUG))
                .show(ui, |ui| {
                    if search.rows.len() == 0 {
                        no_match(ui, theme, &search.text);
                    }
                    // With no query, one run a category, each under its caption; with one,
                    // a single ranked run, the best match at the top.
                    for (start, rows) in search.rows.runs() {
                        if search.rows.grouped()
                            && let Some(first) = rows.iter().find_map(|row| row.entry(library))
                        {
                            caption(ui, theme, first.category());
                        }
                        // Indexed before the lookup, so a row's index stays the one Enter
                        // reads back through `PaletteRows::get`.
                        let entries = rows.iter().enumerate().filter_map(|(offset, row)| {
                            Some((start + offset, row.entry(library)?))
                        });
                        for (idx, entry) in entries {
                            let row = Row {
                                entry,
                                idx,
                                focused: idx == search.focused,
                                query_lc: &search.folded,
                                grouped: search.rows.grouped(),
                            };
                            if row.show(ui, theme) {
                                chosen = Some(entry);
                            }
                        }
                    }
                });
        });
        let chosen = chosen.or_else(|| {
            let row = submitted.then(|| search.rows.get(search.focused))??;
            row.entry(library)
        })?;
        popup.close();
        Some(self.add(chosen))
    }

    /// The `AddNode` a pick of `entry` raises. A library entry and a special node name their
    /// node after the same `Func` and seed the same default bindings from it, so only the
    /// `Node` value itself differs — and it is built only on a pick.
    fn add(&self, entry: PaletteEntry<'_>) -> GraphIntent {
        let node_id = NodeId::unique();
        GraphIntent::AddNode {
            pos: self.pos,
            node_id,
            node: entry.node(),
            bindings: entry.func().default_bindings(node_id).collect(),
        }
    }
}

/// The list's width: room for a long name and its category on one line.
const LIST_WIDTH: f32 = 380.0;

/// One row of the list: the name, with the part the query matched in the accent ink, the
/// category at the right while a query ranks the list, and the description on a second line.
/// The name is its own text, which a text locator names the row by.
#[derive(Debug)]
struct Row<'a> {
    entry: PaletteEntry<'a>,
    idx: usize,
    focused: bool,
    query_lc: &'a str,
    grouped: bool,
}

impl Row<'_> {
    /// Record the row; `true` when it was clicked.
    fn show(self, ui: &mut Ui, theme: &Theme) -> bool {
        let wid = row_wid(self.idx);
        let hovered = ui.response_for(wid).hovered();
        let mut chrome = Chrome {
            corners: Corners::all(theme.radius.control),
            ..Chrome::default()
        };
        if self.focused {
            chrome.fill = focus_fill(theme).into();
        } else if hovered {
            // A menu row's hover, so the palette and the menus light a row alike.
            chrome.fill = ui
                .theme()
                .context_menu
                .item
                .looks
                .hovered
                .chrome
                .fill
                .clone();
        }
        // On a lit row the secondary text takes the full ink, as VS Code's quick pick gives
        // its focused row's description `list.activeSelectionForeground`: the muted ink
        // falls under 4.5:1 on any fill above the panel's.
        let lit = self.focused || hovered;
        let secondary = if lit {
            support::sized_text(ui, theme.text.label)
        } else {
            support::muted_text(ui, theme, theme.text.label)
        };
        let row = Panel::hstack()
            .id(wid)
            .size((Sizing::FILL, Sizing::HUG))
            .child_align(Align::v(VAlign::Stretch))
            .sense(Sense::CLICK)
            .chrome(chrome)
            .show(ui, |ui| {
                let bar = if self.focused {
                    theme.colors.selection_rect
                } else {
                    RgbaF32::TRANSPARENT
                };
                Panel::hstack()
                    .id_salt("focus_bar")
                    .size((Sizing::fixed(FOCUS_BAR_WIDTH), Sizing::HUG))
                    .chrome(Chrome::fill(bar))
                    .show(ui, |_| {});
                Panel::vstack()
                    .id_salt("body")
                    .size((Sizing::FILL, Sizing::HUG))
                    .padding(Spacing::new(
                        theme.space.sm - FOCUS_BAR_WIDTH,
                        theme.space.xs,
                        theme.space.sm,
                        theme.space.xs,
                    ))
                    .show(ui, |ui| {
                        Panel::hstack()
                            .id_salt("line")
                            .size((Sizing::FILL, Sizing::HUG))
                            .child_align(Align::v(VAlign::Center))
                            .show(ui, |ui| {
                                name(ui, theme, self.entry.name(), self.query_lc);
                                support::hspacer(ui, "category_spacer");
                                if !self.grouped {
                                    Text::new(self.entry.category()).style(&secondary).show(ui);
                                }
                            });
                        if let Some(description) = self.entry.description() {
                            Text::new(description)
                                .id_salt("description")
                                .style(&secondary)
                                .text_wrap(TextWrap::Ellipsis)
                                .size((Sizing::FILL, Sizing::HUG))
                                .show(ui);
                        }
                    });
            })
            .response
            .snapshot();
        row.left.clicked()
    }
}

/// The tint behind the focused row: the selection's hue, light enough that the text on it
/// keeps 4.5:1.
const FOCUS_ALPHA: f32 = 0.14;

/// The accent bar on the focused row's left edge, which marks it where the tint is faint.
const FOCUS_BAR_WIDTH: f32 = 2.0;

/// The focused row's fill.
pub(super) const fn focus_fill(theme: &Theme) -> RgbaF32 {
    theme.colors.selection_rect.with_alpha(FOCUS_ALPHA)
}

pub(super) fn row_wid(idx: usize) -> WidgetId {
    WidgetId::from_hash(("new_node_row", idx))
}

/// `text` with the first stretch that matches `query_lc` in the accent ink. Names fold `char`
/// by `char` as the query did; a name whose fold changes its length draws plain.
fn name(ui: &mut Ui, theme: &Theme, text: &str, query_lc: &str) {
    let plain = support::sized_text(ui, theme.text.body);
    let lit = TextStyle {
        weight: FontWeight::SEMI_BOLD,
        ..support::colored_text(ui, theme.colors.selection_rect, theme.text.body)
    };
    let span = match_span(text, query_lc);
    Panel::hstack()
        .id_salt("name")
        .size((Sizing::HUG, Sizing::HUG))
        .show(ui, |ui| match span {
            Some(span) => {
                for (part, style) in [
                    (&text[..span.start], &plain),
                    (&text[span.clone()], &lit),
                    (&text[span.end..], &plain),
                ] {
                    if !part.is_empty() {
                        Text::new(part).style(style).show(ui);
                    }
                }
            }
            None => {
                Text::new(text).style(&plain).show(ui);
            }
        });
}

/// The byte range of `name` that the folded `query_lc` matches first, when folding keeps the
/// name's length, as it does for every ASCII name.
fn match_span(name: &str, query_lc: &str) -> Option<Range<usize>> {
    if query_lc.is_empty() || !name.is_ascii() || !query_lc.is_ascii() {
        return None;
    }
    let start = name
        .as_bytes()
        .windows(query_lc.len())
        .position(|window| window.eq_ignore_ascii_case(query_lc.as_bytes()))?;
    Some(start..start + query_lc.len())
}

/// A category's name above its run, in the muted label ink.
fn caption(ui: &mut Ui, theme: &Theme, category: &str) {
    let style = support::muted_text(ui, theme, theme.text.label);
    Text::new(category)
        .style(&style)
        .margin(Spacing::new(
            theme.space.sm,
            theme.space.sm,
            0.0,
            theme.space.xs,
        ))
        .show(ui);
}

/// What the list says when the query matches nothing.
fn no_match(ui: &mut Ui, theme: &Theme, query: &str) {
    let body = support::sized_text(ui, theme.text.body);
    let hint = support::muted_text(ui, theme, theme.text.label);
    Panel::vstack()
        .id_salt("no_match")
        .size((Sizing::FILL, Sizing::HUG))
        .padding(Spacing::all(theme.space.sm))
        .gap(theme.space.xs)
        .show(ui, |ui| {
            Text::new(fmt!(ui, "No function matches \u{201c}{query}\u{201d}."))
                .style(&body)
                .show(ui);
            Text::new("Try another word.").style(&hint).show(ui);
        });
}
