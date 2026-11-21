//! [`NodePalette`]: everything one new-node popup lists, and the rows and
//! columns it records.

use std::ops::Range;

use palantir::{
    Block, Chrome, Configure, Corners, EditEnd, FontWeight, Key, KeyboardWake, MenuSeparator,
    Panel, RgbaF32, Scroll, Sense, SizeSpec, Sizing, Spacing, Text, TextEdit, TextStyle, TextWrap,
    Ui, VAlign, WidgetId, fmt,
};

use glam::Vec2;
use scenarium::{
    Boundary, Compound, CompoundId, Func, GraphId, Library, Node, NodeId, Ports, SPECIAL_NODES,
    SpecialNode, Wiring,
};

use crate::core::document::Document;
use crate::core::edit::bring_in::bring_in;
use crate::core::edit::compound_intent::NewCompound;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::shared_library::{LibraryEntry, SharedLibrary};
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::gesture::new_node_ui;
use crate::gui::pane::graph_ui::gesture::new_node_ui::Search;
use crate::gui::pane::graph_ui::gesture::new_node_ui::landing::{DroppedEnd, Landing};
use crate::gui::pane::graph_ui::gesture::new_node_ui::palette_rows::{PaletteRow, PaletteSources};
use crate::gui::theme::Theme;
use crate::gui::theme::color;
use crate::gui::widgets::support;

/// One row of a category's palette list: a library `Func`, a built-in special node, or one of
/// the document's compounds. One type lets every source sort into one list, which makes a
/// category a run inside it.
#[derive(Clone, Copy, Debug)]
pub(super) enum PaletteEntry<'a> {
    Func(&'a Func),
    Special(SpecialNode),
    /// Whether a node places it: an unused one is marked, as the library list marks it.
    Compound {
        id: CompoundId,
        compound: &'a Compound,
        used: bool,
    },
    /// A compound the library folder shares, which a pick copies in with those it places.
    Library {
        entry: &'a LibraryEntry,
        shared: &'a SharedLibrary,
    },
    /// Another `Inputs` node, listed in a compound's pane alone, so a long graph reads an input
    /// where it is used.
    Inputs,
}

/// The category the document's compounds list under.
const COMPOUNDS: &str = "Compounds";
/// The category the library folder's compounds list under.
const LIBRARY: &str = "Library";
/// The category an `Inputs` node lists under, beside the compound it shows.
const INTERFACE: &str = "Interface";
/// The ports a node with none to seed shows: an `Inputs` node, whose ports are outputs.
static NO_PORTS: Ports = Ports {
    inputs: Vec::new(),
    outputs: Vec::new(),
};

impl<'a> PaletteEntry<'a> {
    /// Borrowed from the palette's sources rather than from `self`, so a
    /// name outlives any borrow of the row that yielded it.
    pub(super) fn name(self) -> &'a str {
        match self {
            PaletteEntry::Func(f) => f.name(),
            PaletteEntry::Special(s) => s.func().name(),
            PaletteEntry::Compound { compound, .. } => &compound.name,
            PaletteEntry::Library { entry, .. } => &entry.name,
            PaletteEntry::Inputs => "Inputs",
        }
    }

    pub(super) fn category(self) -> &'a str {
        match self {
            PaletteEntry::Func(f) => f.category(),
            PaletteEntry::Special(s) => s.func().category(),
            PaletteEntry::Compound { .. } => COMPOUNDS,
            PaletteEntry::Library { .. } => LIBRARY,
            PaletteEntry::Inputs => INTERFACE,
        }
    }

    /// The first line of what the entry does, if it says; an unused compound says so instead.
    pub(super) fn description(self) -> Option<&'a str> {
        let text = match self {
            PaletteEntry::Func(f) => f.description(),
            PaletteEntry::Special(s) => s.func().description(),
            PaletteEntry::Compound { used: false, .. } => Some("Unused: no node places it"),
            PaletteEntry::Compound { compound, .. } => compound.description.as_deref(),
            // Its file names it apart from an entry of another file under the same id.
            PaletteEntry::Library { entry, shared } => shared
                .path(entry)
                .file_name()
                .and_then(|name| name.to_str()),
            PaletteEntry::Inputs => Some("The compound's inputs, read again where they are used"),
        }?;
        Some(text.lines().next().unwrap_or(text))
    }

    /// The ports a node of this entry shows, whose defaults its new node takes.
    pub(super) fn ports(self) -> &'a Ports {
        match self {
            PaletteEntry::Func(f) => f.ports(),
            PaletteEntry::Special(s) => s.func().ports(),
            PaletteEntry::Compound { compound, .. } => compound.interface.ports(),
            PaletteEntry::Library { entry, shared } => shared
                .document(entry)
                .graphs
                .compound(entry.id)
                .expect("an entry names a compound of its file")
                .interface
                .ports(),
            PaletteEntry::Inputs => &NO_PORTS,
        }
    }

    /// What a pick of this row adds to `document`: the node, and for a library entry the
    /// definitions it copies in first. A special node's `Func` is hardcoded rather than
    /// library-registered, so the node it spawns comes from the [`SpecialNode`] itself.
    fn placed(self, document: &Document, library: &Library) -> Placed {
        let alone = |node| Placed {
            compounds: Vec::new(),
            node,
        };
        match self {
            PaletteEntry::Func(f) => alone(f.into()),
            PaletteEntry::Special(s) => alone(s.into()),
            PaletteEntry::Compound { id, .. } => alone(Node::instance(id)),
            PaletteEntry::Inputs => alone(Node::from(Boundary::Inputs)),
            PaletteEntry::Library { entry, shared } => {
                let brought = bring_in(
                    document,
                    library,
                    shared.carried(entry),
                    shared.manifest(entry),
                );
                let node = Node::instance(brought.id(entry.id));
                Placed {
                    compounds: brought.compounds,
                    node,
                }
            }
        }
    }

    /// The identity
    /// [`PaletteRows`](crate::gui::pane::graph_ui::gesture::new_node_ui::palette_rows::PaletteRows)
    /// keeps for this entry.
    pub(super) const fn row(self) -> PaletteRow {
        match self {
            PaletteEntry::Func(f) => PaletteRow::Func(f.id()),
            PaletteEntry::Special(s) => PaletteRow::Special(s),
            PaletteEntry::Compound { id, .. } => PaletteRow::Compound(id),
            PaletteEntry::Library { entry, .. } => PaletteRow::Library {
                id: entry.id,
                digest: entry.digest,
            },
            PaletteEntry::Inputs => PaletteRow::Inputs,
        }
    }
}

/// The node a pick adds, and the definitions it copies in first.
#[derive(Debug)]
struct Placed {
    compounds: Vec<NewCompound>,
    node: Node,
}

/// What a palette pick raises, in order: the definitions it copies in, the node it adds, and the
/// dropped wire laid onto the port its row names.
#[derive(Debug)]
pub(super) struct PalettePick {
    pub(super) compounds: Vec<NewCompound>,
    pub(super) add: GraphIntent,
    pub(super) wire: Option<GraphIntent>,
}

/// Everything one palette open lists, plus where a pick lands and how the frame lays it out,
/// built once per body so the helpers take one borrow.
#[derive(Debug)]
pub(super) struct NodePalette<'a> {
    pub(super) graph_ctx: GraphCtx<'a>,
    /// World position the open captured; every intent a row raises places its node here.
    pub(super) pos: Vec2,
    /// The most height the results may scroll in.
    pub(super) scroll_cap: f32,
    pub(super) opening: Opening,
    /// Opened to pick a node that replaces another: the library folder's entries, which a pick
    /// copies in first, and an `Inputs` node are not listed.
    pub(super) replacing: bool,
    /// The end of the wire whose drop opened the palette: it lists the ports that take it.
    pub(super) dropped: Option<DroppedEnd<'a>>,
}

/// Whether the palette opened this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Opening {
    /// Opened this frame: the search field takes the focus, and the rows are
    /// filtered afresh against the library as it is now.
    Fresh,
    /// Open since an earlier frame.
    #[default]
    Standing,
}

impl<'a> NodePalette<'a> {
    /// What the rows are read from.
    const fn sources(&self) -> PaletteSources<'a> {
        PaletteSources {
            library: self.graph_ctx.library(),
            graphs: &self.graph_ctx.document().graphs,
            shared: self.graph_ctx.shared(),
        }
    }

    /// Every row the palette can list, in no particular order: the library's
    /// funcs, the built-in specials, the document's compounds, and unless it replaces a node the
    /// library folder's and, in a compound's pane, an `Inputs` node.
    pub(super) fn entries(&self) -> impl Iterator<Item = PaletteEntry<'a>> {
        let sources = self.sources();
        let replacing = self.replacing;
        let inputs = (!replacing && matches!(self.graph_ctx.graph(), GraphId::Compound(_)))
            .then_some(PaletteEntry::Inputs);
        sources
            .library
            .iter()
            .map(PaletteEntry::Func)
            .chain(SPECIAL_NODES.iter().copied().map(PaletteEntry::Special))
            .chain(
                sources
                    .graphs
                    .compounds()
                    .filter_map(move |(id, _)| PaletteRow::Compound(id).entry(sources)),
            )
            .chain(
                sources
                    .shared
                    .entries()
                    .iter()
                    .filter(move |_| !replacing)
                    .map(move |entry| PaletteEntry::Library {
                        entry,
                        shared: sources.shared,
                    }),
            )
            .chain(inputs)
    }
}

impl NodePalette<'_> {
    /// Record the search field and the rows `search` matches, and move or pick the focused row
    /// on Up, Down and Enter.
    pub(super) fn body(&self, ui: &mut Ui, search: &mut Search) -> Option<PalettePick> {
        let opened = self.opening == Opening::Fresh;
        let theme = self.graph_ctx.theme();
        let search_id = new_node_ui::search_field_wid();
        let sources = self.sources();
        // The search row: the field on the rows' left edge, and what the keys do at the right.
        let mut submitted = false;
        let mut reveal = opened;
        Panel::hstack()
            .id(new_node_ui::search_row_wid())
            .size((Sizing::fill(1.0), Sizing::HUG))
            .min_size((LIST_WIDTH, 0.0))
            .padding(Spacing::xy(theme.space.sm, theme.space.xs))
            .gap(theme.space.md)
            .child_align(VAlign::Center)
            .show(ui, |ui| {
                let field = TextEdit::new(&mut search.text)
                    .id(search_id)
                    // The field filters the palette rather than editing a value, so Esc closes
                    // the whole popup instead of blurring the only thing in it.
                    .escape_falls_through(true)
                    .placeholder("Search…")
                    .style(&theme.flat_text_edit)
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
                        .refilter(sources, self.entries(), &search.folded, self.dropped);
                    search.focused = 0;
                    reveal = true;
                }
                let hint = if search.rows.len() == 0 {
                    "esc close"
                } else {
                    "\u{21b5} add \u{b7} \u{2191}\u{2193} move"
                };
                let style = support::muted_text(ui, theme, theme.font.caption);
                Text::new(hint).style(&style).show(ui);
            });
        MenuSeparator::new().show(ui);
        // Read off the popup's own presses rather than `key_pressed`: the search field holds the
        // focus and its scope takes the arrow keys, which a single-line field leaves unused.
        let last = search.rows.len().saturating_sub(1);
        for press in ui.keyboard_events(KeyboardWake::KEY) {
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
            .size(SizeSpec::HUG)
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
                            && let Some(first) =
                                rows.iter().find_map(|listed| listed.row.entry(sources))
                        {
                            caption(ui, theme, first.category());
                        }
                        // Indexed before the lookup, so a row's index stays the one Enter
                        // reads back through `PaletteRows::get`.
                        let entries = rows.iter().enumerate().filter_map(|(offset, listed)| {
                            Some((start + offset, listed.row.entry(sources)?, listed.landing))
                        });
                        for (idx, entry, landing) in entries {
                            let row = Row {
                                entry,
                                landing: landing.zip(self.dropped).map(|(landing, dropped)| {
                                    dropped.landing_name(entry.ports(), landing)
                                }),
                                idx,
                                focused: idx == search.focused,
                                query_lc: &search.folded,
                                grouped: search.rows.grouped(),
                            };
                            if row.show(ui, theme) {
                                chosen = Some((entry, landing));
                            }
                        }
                    }
                });
        });
        let (entry, landing) = chosen.or_else(|| {
            let listed = submitted.then(|| search.rows.get(search.focused))??;
            Some((listed.row.entry(sources)?, listed.landing))
        })?;
        ui.close_overlay();
        Some(self.add(entry, landing))
    }

    /// What a pick of `entry` raises: the definitions a library entry copies in, then the
    /// `AddNode`, seeded with the entry's port defaults, then the dropped wire onto `landing`.
    /// Built only on a pick. A wire whose source an undo took since the drop is laid nowhere.
    pub(super) fn add(&self, entry: PaletteEntry<'_>, landing: Option<Landing>) -> PalettePick {
        let node_id = NodeId::unique();
        let Placed { compounds, node } =
            entry.placed(self.graph_ctx.document(), self.graph_ctx.library());
        PalettePick {
            compounds,
            add: GraphIntent::AddNode {
                pos: self.pos,
                node_id,
                node,
                bindings: Wiring::defaults(node_id, entry.ports()),
            },
            wire: landing
                .zip(self.dropped)
                .map(|(landing, dropped)| dropped.wire(node_id, landing)),
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
    /// The name of the port of the new node a dropped wire lands on, which the row names after
    /// the entry.
    landing: Option<&'a str>,
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
            chrome.paint.fill = focus_fill(theme).into();
        } else if hovered {
            // A menu row's hover, so the palette and the menus light a row alike.
            chrome.paint.fill = ui
                .theme()
                .menu
                .item
                .unselected
                .hovered
                .chrome
                .paint
                .fill
                .clone();
        }
        // On a lit row the secondary text takes the full ink, as VS Code's quick pick gives
        // its focused row's description `list.activeSelectionForeground`: the muted ink
        // falls under 4.5:1 on any fill above the panel's.
        let lit = self.focused || hovered;
        let secondary = if lit {
            support::sized_text(ui, theme.font.caption)
        } else {
            support::muted_text(ui, theme, theme.font.caption)
        };
        let row = Panel::hstack()
            .id(wid)
            .size(SizeSpec::FILL_W)
            .child_align(VAlign::Stretch)
            .sense(Sense::CLICK)
            .chrome(chrome)
            .show(ui, |ui| {
                let bar = if self.focused {
                    theme.colors.accent
                } else {
                    RgbaF32::TRANSPARENT
                };
                Block::new()
                    .id_salt("focus_bar")
                    .size((Sizing::fixed(theme.stroke.line), Sizing::HUG))
                    .chrome(Chrome::new(bar))
                    .show(ui);
                Panel::vstack()
                    .id_salt("body")
                    .size(SizeSpec::FILL_W)
                    .padding(Spacing::new(
                        theme.space.sm - theme.stroke.line,
                        theme.space.xs,
                        theme.space.sm,
                        theme.space.xs,
                    ))
                    .show(ui, |ui| {
                        Panel::hstack()
                            .id_salt("line")
                            .size(SizeSpec::FILL_W)
                            .child_align(VAlign::Center)
                            .show(ui, |ui| {
                                name(ui, theme, self.entry.name(), self.query_lc, lit);
                                if let Some(port) = self.landing {
                                    let style = support::body_text(ui, theme);
                                    Text::new(fmt!(ui, " \u{25b8} {port}"))
                                        .id_salt("landing")
                                        .style(&style)
                                        .show(ui);
                                }
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
                                .size(SizeSpec::FILL_W)
                                .show(ui);
                        }
                    });
            })
            .response;
        row.clicked()
    }
}

/// The focused row's fill: a soft wash of the selection's hue over the popup, whose panel is a
/// node's fill. An accent bar on its left edge marks it where the tint is faint.
pub(super) fn focus_fill(theme: &Theme) -> RgbaF32 {
    color::wash(theme.colors.accent, theme.tint.soft, theme.card.fill)
}

pub(super) fn row_wid(idx: usize) -> WidgetId {
    WidgetId::from_hash(("darkroom.palette.row", idx))
}

/// `text` with the first stretch that matches `query_lc` in semibold, in the accent ink on a
/// resting row and in the body ink on a `lit` one, where the accent falls under 4.5:1. Names
/// fold `char` by `char` as the query did; a name whose fold changes its length draws plain.
fn name(ui: &mut Ui, theme: &Theme, text: &str, query_lc: &str, lit: bool) {
    let plain = support::body_text(ui, theme);
    let matched = TextStyle {
        weight: FontWeight::SEMI_BOLD,
        color: if lit {
            plain.color
        } else {
            theme.colors.accent
        },
        ..plain
    };
    let span = match_span(text, query_lc);
    Panel::hstack()
        .id_salt("name")
        .size(SizeSpec::HUG)
        .show(ui, |ui| match span {
            Some(span) => {
                for (part, style) in [
                    (&text[..span.start], &plain),
                    (&text[span.clone()], &matched),
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

/// A category's name above its run, as a caption.
fn caption(ui: &mut Ui, theme: &Theme, category: &str) {
    let style = support::caption_text(ui, theme);
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
    let body = support::body_text(ui, theme);
    let hint = support::muted_text(ui, theme, theme.font.caption);
    Panel::vstack()
        .id_salt("no_match")
        .size(SizeSpec::FILL_W)
        .padding(Spacing::all(theme.space.sm))
        .gap(theme.space.xs)
        .show(ui, |ui| {
            Text::new(fmt!(ui, "No function matches \u{201c}{query}\u{201d}."))
                .style(&body)
                .show(ui);
            Text::new("Try another word.").style(&hint).show(ui);
        });
}
