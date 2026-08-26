//! The dock's GUI half: pane tree rendering, per-group tab strips,
//! divider resizing, and tab drag-and-drop — everything between the
//! persisted model (`core::document::dock`) and the pane *content*,
//! which stays the caller's. [`DockUi`] is the whole integration
//! surface, two calls wide:
//!
//! - [`DockUi::scan`] in the navigation phase — moves focus to the pane
//!   the pointer pressed in, surfaces tab activate/close clicks, and
//!   drives the drag lifecycle off last frame's responses, queueing a
//!   [`DockOp`] for each onto the frame's sink.
//! - [`DockUi::render`] in the record — walks the split tree (splits as
//!   palantir `Splitter`s whose ratio drags surface as
//!   `DockOp::SetRatio`, groups as strip-over-content panes) and,
//!   mid-drag, paints the drop-zone highlight + ghost chip and holds
//!   the grabbing cursor. The `content` closure renders the active
//!   tab's view, so this module never learns what a canvas or a viewer
//!   is.
//!
//! Submodules: `strip` (the chip row) and `drag` (gesture state + the
//! pure pointer→drop-zone math).

mod drag;
pub(crate) mod strip;

use crate::core::document::dock::dock_op::DockOp;
use crate::core::document::dock::dock_path::DockPath;
use crate::core::document::dock::split_side::SplitDir;
use crate::core::document::dock::tab_group::TabGroup;
use crate::core::document::dock::{DockLayout, DockNode, DockSplit, NodeIdx, TabGroupId};
use crate::core::document::open_document::OpenDocument;
use crate::core::document::{Document, TabRef};
use crate::gui::dock::drag::{DropTarget, PaneGeometry, TabDrag, classify_drop};
use crate::gui::dock::strip::TabLabel;
use crate::gui::pane::viewer;
use crate::gui::requests::Requests;
use crate::gui::theme::Theme;
use crate::gui::widgets::support::sized_text;
use common::FloatExt;
use glam::Vec2;
use palantir::{
    Background, Configure, Corners, CursorIcon, Layer, Panel, Rect, Sizing, Spacing, SplitHalf,
    Splitter, Stroke, Text, Ui, WidgetId,
};

/// Smallest a dock pane can be squeezed on its split axis, in logical px.
const MIN_PANE: f32 = 220.0;

/// Stable id for a group's pane container — the rect the drop-zone math
/// keys off.
fn pane_wid(group: TabGroupId) -> WidgetId {
    WidgetId::from_hash(("dock.pane", group))
}

/// Stable id for a group's *content* area — the space below the strip that
/// the active tab's view fills.
///
/// Keyed by the group rather than by the tab it happens to be showing, which
/// is the whole point: switching tabs leaves this widget in place, so a view
/// can be handed its arranged size on the very pass it first records. A view
/// measuring its own container instead would find nothing there — its widget
/// is one pass old at most, and layout runs after the record.
fn content_wid(group: TabGroupId) -> WidgetId {
    WidgetId::from_hash(("dock.content", group))
}

/// Stable id for the splitter at a tree path.
fn splitter_wid(path: DockPath) -> WidgetId {
    WidgetId::from_hash(("dock.splitter", path))
}

/// Stable id for the in-flight drag's drop-zone highlight.
fn drag_highlight_wid() -> WidgetId {
    WidgetId::from_hash("dock.drag_highlight")
}

/// Stable id for the ghost chip trailing the pointer mid-drag.
fn drag_ghost_wid() -> WidgetId {
    WidgetId::from_hash("dock.drag_ghost")
}

/// What the render walk needs from outside the layout: the open document it
/// is arranging — its panes, and the unsaved-changes flag the strip shows —
/// and the palette it paints with. Carried as one value rather than two
/// parameters so the recursive render keeps its arity.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DockContext<'a> {
    pub(crate) open: &'a OpenDocument,
    pub(crate) theme: &'a Theme,
}

/// The dock's persistent GUI state — just the drag in flight; the
/// arrangement itself lives on the `Document`.
#[derive(Debug, Default)]
pub(crate) struct DockUi {
    /// Armed by [`Self::scan`] off a chip's latched drag,
    /// resolved there into a [`DockOp::MoveTab`] on release (or
    /// cancelled by Esc), painted by [`Self::render`].
    tab_drag: Option<TabDrag>,
    /// [`tab_labels`]' buffer. One serves every group, because the borrow
    /// checker holds it for exactly as long as a group's strip reads it.
    tab_labels: Vec<TabLabel>,
}

impl DockUi {
    /// Navigation-phase scan: [`scan_focus`] first (focus follows a press
    /// into a pane), then one pass over every strip's last-frame chip
    /// responses — close clicks (which win over activation), activation
    /// clicks, drag arming off the tab's chip — then the in-flight drag's
    /// lifecycle: cancel on Esc (or the tab vanishing under it), and on
    /// release resolve the pane under the pointer into a
    /// [`DockOp::MoveTab`].
    ///
    /// Scanning in the *prepass* (not as record-time pushes) is
    /// load-bearing: the navigation phase settles the new arrangement
    /// before this frame's record, so a switch — or a committed drop —
    /// draws the same frame it lands.
    pub(crate) fn scan(&mut self, ui: &mut Ui, doc: &Document, out: &mut Requests) {
        // Ahead of the chip pass: a read-only focus query, and one that only
        // ever moves `focused`, so it composes with an activation from the
        // same scan rather than racing it.
        scan_focus(ui, doc, out);
        for tab in doc.layout.all_tabs() {
            if strip::closable(tab) && ui.response_for(strip::tab_close_wid(tab)).left.clicked() {
                out.push_view(DockOp::CloseTab { tab });
                continue;
            }
            if ui.response_for(strip::tab_chip_wid(tab)).left.clicked() {
                out.push_view(DockOp::ActivateTab { tab });
            }
            if self.tab_drag.is_none()
                && ui
                    .response_for(strip::tab_chip_wid(tab))
                    .left
                    .drag
                    .started()
            {
                self.tab_drag = Some(TabDrag {
                    tab,
                    text: tab_text(doc, tab).to_owned(),
                });
            }
        }
        let Some(dragged) = &self.tab_drag else {
            return;
        };
        let tab = dragged.tab;
        if ui.escape_pressed() || doc.layout.find_tab(tab).is_none() {
            self.tab_drag = None;
            return;
        }
        // The release edge fires on the chip that caught the press.
        if ui
            .response_for(strip::tab_chip_wid(tab))
            .left
            .drag
            .stopped()
        {
            if let Some(target) = drop_target(ui, doc) {
                out.push_view(DockOp::MoveTab {
                    tab,
                    to: target.drop,
                });
            }
            self.tab_drag = None;
        }
    }

    /// Record the dock: the split tree, each group's strip over its
    /// active tab's view (rendered by `content` — called once per
    /// visible group with the tab and the frame's intent sink), and the
    /// in-flight drag's feedback. Ratio drags and strip-borne intents
    /// (renames, split-menu picks) land in `out`.
    pub(crate) fn render(
        &mut self,
        ui: &mut Ui,
        cx: DockContext<'_>,
        out: &mut Requests,
        mut content: impl FnMut(&mut Ui, TabRef, Option<Vec2>, &mut Requests),
    ) {
        let Self {
            tab_drag,
            tab_labels,
        } = self;
        render_node(
            ui,
            cx,
            DockLayout::ROOT,
            DockPath::ROOT,
            out,
            &mut content,
            tab_labels,
        );
        if let Some(dragged) = tab_drag {
            ui.set_cursor(CursorIcon::Grabbing);
            draw_drag_feedback(ui, cx, dragged);
        }
    }
}

/// Recursive walk of the dock tree: a split renders as an palantir
/// `Splitter` (ratio changes surface as `DockOp::SetRatio`), a
/// group as its strip + the active tab's view.
fn render_node<F: FnMut(&mut Ui, TabRef, Option<Vec2>, &mut Requests)>(
    ui: &mut Ui,
    cx: DockContext<'_>,
    idx: NodeIdx,
    path: DockPath,
    out: &mut Requests,
    content: &mut F,
    labels: &mut Vec<TabLabel>,
) {
    match cx.open.document.layout.node(idx) {
        DockNode::Group(group) => render_group(ui, cx, group, out, content, labels),
        DockNode::Split(split) => {
            let DockSplit {
                dir,
                ratio,
                first,
                second,
            } = *split;
            let mut live_ratio = ratio;
            let splitter = match dir {
                SplitDir::Row => Splitter::horizontal(&mut live_ratio),
                SplitDir::Column => Splitter::vertical(&mut live_ratio),
            };
            splitter
                .id(splitter_wid(path))
                .min_pane(MIN_PANE)
                .show(ui, |ui, half| {
                    let (child, child_path) = match half {
                        SplitHalf::First => (first, path.first()),
                        SplitHalf::Second => (second, path.second()),
                    };
                    render_node(ui, cx, child, child_path, out, content, labels);
                });
            // The widget wrote the divider drag into `live_ratio`; the
            // layout itself only changes through the recorded intent
            // (drained post-record, coalescing per divider). Approximate
            // compare for the same reason `pan_zoom::emit_pan_zoom` uses
            // one — an exact `!=` emits on the last-bit noise a re-derived
            // ratio carries.
            if !live_ratio.approximately_eq(ratio) {
                out.push_view(DockOp::SetRatio {
                    split: path,
                    ratio: live_ratio,
                });
            }
        }
    }
}

/// One pane: the group's tab strip over its active tab's view.
fn render_group<F: FnMut(&mut Ui, TabRef, Option<Vec2>, &mut Requests)>(
    ui: &mut Ui,
    cx: DockContext<'_>,
    group: &TabGroup,
    out: &mut Requests,
    content: &mut F,
    labels: &mut Vec<TabLabel>,
) {
    tab_labels(ui, cx, group, labels);
    Panel::vstack()
        .id(pane_wid(group.id))
        .size((Sizing::FILL, Sizing::FILL))
        // Focusable so a press anywhere in the pane that misses every inner
        // focusable lands here — what [`scan_focus`] reads to move dock
        // focus. Never a keyboard target in its own right: keys route by
        // input scope, and a pane declares none.
        .focusable(true)
        .show(ui, |ui| {
            strip::show(ui, cx.theme, group, labels, out);
            // Last frame's arrangement, as every measurement during a record
            // is — but of the *group's* content area, which outlives the tab
            // in it. That is what lets a view that first records on this pass
            // still be handed a size.
            let size = content_size(ui, group.id);
            Panel::vstack()
                .id(content_wid(group.id))
                .size((Sizing::FILL, Sizing::FILL))
                .show(ui, |ui| content(ui, group.active_tab(), size, out));
        });
}

/// The arranged size of a group's content area, `None` before its first
/// layout — the one frame in a group's life where a view has to size itself.
fn content_size(ui: &Ui, group: TabGroupId) -> Option<Vec2> {
    let size = ui.response_for(content_wid(group)).layout_rect?.size;
    (size.w > 0.0 && size.h > 0.0).then(|| Vec2::new(size.w, size.h))
}

/// Dock focus follows keyboard focus into a pane: whichever pane holds it
/// becomes the focused group.
///
/// [`DockUi::scan`]'s chip pass only covers presses on a *strip*. A press in
/// a pane's **content** — a node body, bare canvas, an image viewer —
/// reaches no dock widget at all, so without this the focus stays wherever
/// the last chip click left it while the user works in another pane, and
/// everything scoped to the focused group (Delete, Ctrl+D, Esc, the Run and
/// node-menu commands) acts on a pane that isn't under the cursor.
///
/// Panes are recorded `focusable`, so palantir's own left-press focus
/// hit-test does the routing: a press that misses every focusable inside a
/// pane lands on the pane container, and a press that lands on a `TextEdit`
/// in there focuses the field while [`Ui::focus_within`] still answers for
/// the pane. Both are the same question — "which pane did the user reach
/// into" — and asking it this way inherits occlusion, clipping, layers, and
/// left-button-only for free. A press outside every pane (the menu bar, the
/// status bar) focuses nothing and leaves the dock focus alone.
///
fn scan_focus(ui: &Ui, doc: &Document, out: &mut Requests) {
    if let Some(group) = doc
        .layout
        .groups()
        .find(|g| g.id != doc.layout.focused && ui.focus_within(pane_wid(g.id)))
    {
        out.push_view(DockOp::FocusPane { group: group.id });
    }
}

/// The drop the pointer currently indicates: the pane whose rect
/// contains it (panes tile the dock area without overlapping, so plain
/// containment against last-frame rects is exact), classified into a
/// zone. Deliberately *not* `hover_within`: the hover hit-test resolves
/// only to sensed widgets, and a pane's content can be entirely inert
/// (the preferences form, a viewer's image) — the pointer over it
/// hovers nothing, and the drop would go dark. `None` over a divider,
/// the chrome rows, or off-window — a release there cancels.
fn drop_target(ui: &mut Ui, doc: &Document) -> Option<DropTarget> {
    let p = ui.pointer_pos()?;
    for group in doc.layout.groups() {
        let Some(pane) = ui.response_for(pane_wid(group.id)).rect else {
            continue;
        };
        if !pane.contains(p) {
            continue;
        }
        let Some(strip_rect) = ui.response_for(strip::strip_wid(group.id)).rect else {
            continue;
        };
        let chips: Vec<Rect> = group
            .tabs
            .iter()
            .filter_map(|&tab| ui.response_for(strip::tab_chip_wid(tab)).rect)
            .collect();
        return Some(classify_drop(
            PaneGeometry {
                group: group.id,
                pane,
                strip: strip_rect,
                chips: &chips,
                can_split: doc.layout.can_split(group.id),
            },
            p,
        ));
    }
    None
}

/// The drag's tooltip-layer feedback: a translucent accent rect over
/// the region the drop would occupy (full pane for a join, half for a
/// split, a caret between chips for a strip insert) and a small ghost
/// chip trailing the pointer. `Sense::NONE` throughout, so the overlay
/// never intercepts the drag's own hit-testing.
fn draw_drag_feedback(ui: &mut Ui, cx: DockContext<'_>, dragged: &TabDrag) {
    let theme = cx.theme;
    let accent = theme.colors.selection_rect;
    if let Some(target) = drop_target(ui, &cx.open.document) {
        let r = target.highlight;
        ui.layer(Layer::Tooltip)
            .at(r.min)
            .max_size(r.size)
            .show(|ui| {
                Panel::zstack()
                    .id(drag_highlight_wid())
                    .size((Sizing::FILL, Sizing::FILL))
                    .background(
                        Background::rounded(accent.with_alpha(0.18), Corners::all(2.0))
                            .with_stroke(Stroke::solid(accent, 1.5)),
                    )
                    .show(ui, |_| {});
            });
    }
    if let Some(p) = ui.pointer_pos() {
        let text = dragged.text.as_str();
        let label_style = sized_text(ui, theme.text.body);
        ui.layer(Layer::Tooltip)
            .at(p + Vec2::new(14.0, 18.0))
            .show(|ui| {
                Panel::hstack()
                    .id(drag_ghost_wid())
                    .size((Sizing::HUG, Sizing::HUG))
                    .padding(Spacing::new(10.0, 4.0, 10.0, 4.0))
                    .background(
                        Background::rounded(theme.colors.chrome_fill, Corners::all(4.0))
                            .with_stroke(Stroke::solid(accent, 1.0)),
                    )
                    .show(ui, |ui| {
                        Text::new(text).style(&label_style).show(ui);
                    });
            });
    }
}

/// Project one group's tabs into `out` as the strip's per-tab labels — the
/// label text and the unsaved-changes flag are what the strip needs the open
/// document for. `out` is cleared first.
///
/// Fills a caller-owned buffer rather than returning one because the dock
/// records every frame: a fresh `Vec` here would be one allocation per
/// visible group per frame, and the labels themselves hold no owned text.
fn tab_labels(ui: &mut Ui, cx: DockContext<'_>, group: &TabGroup, out: &mut Vec<TabLabel>) {
    let doc = &cx.open.document;
    let focused = doc.layout.focused == group.id;
    out.clear();
    out.reserve_exact(group.tabs.len());
    out.extend(group.tabs.iter().enumerate().map(|(i, &tab)| TabLabel {
        tab,
        text: ui.intern(tab_text(doc, tab)),
        active: i == group.active,
        focused,
        dirty: cx.open.dirty,
    }));
}

/// A tab's display text — shared by the strip labels, the drag's ghost chip,
/// and the viewer pane's own header.
fn tab_text(doc: &Document, tab: TabRef) -> &str {
    match tab {
        TabRef::Graph => "main",
        TabRef::Preferences => "preferences",
        TabRef::ImageViewer(node_id) => viewer::node_label(doc, node_id),
    }
}
