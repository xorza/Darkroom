//! The dock's GUI half: pane tree rendering, per-group tab strips,
//! divider resizing, and tab drag-and-drop — everything between the
//! persisted model (`core::document::dock`) and the pane *content*,
//! which stays the caller's. [`DockUi`] is the whole integration
//! surface, two calls wide:
//!
//! - [`DockUi::scan`] in the navigation phase — moves focus to the pane
//!   the pointer pressed in, surfaces tab activate/close clicks, and
//!   drives the drag lifecycle off last frame's responses, emitting
//!   `UiAction`s.
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

use std::borrow::Cow;
use std::collections::HashMap;

use common::FloatExt;
use glam::Vec2;
use palantir::{
    Background, Configure, Corners, CursorIcon, Layer, Panel, Rect, Sizing, Spacing, SplitHalf,
    Splitter, Stroke, Text, Ui, WidgetId,
};
use scenarium::NodeId;

use crate::core::document::dock::{
    DockLayout, DockNode, DockOp, DockPath, DockSplit, NodeIdx, SplitDir, TabGroup, TabGroupId,
};
use crate::core::document::{Document, TabRef};
use crate::core::edit::intent::sink::Intents;
use crate::gui::UiAction;
use crate::gui::dock::drag::{DropTarget, PaneGeometry, TabDrag, classify_drop};
use crate::gui::dock::strip::TabLabel;
use crate::gui::pane::viewer;
use crate::gui::theme::Theme;
use crate::gui::widgets::support::sized_text;

/// Smallest a dock pane can be squeezed on its split axis, in logical px.
const MIN_PANE: f32 = 220.0;

/// Stable id for a group's pane container — the rect the drop-zone math
/// keys off.
fn pane_wid(group: TabGroupId) -> WidgetId {
    WidgetId::from_hash(("dock.pane", group))
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

/// What the render walk needs from outside the layout: the document, plus
/// this frame's viewer-tab labels resolved once by the caller.
///
/// [`viewer::node_label`] runs a recursive whole-document node search
/// and allocates a `String`, and a visible viewer tab is labelled twice a
/// frame — once for its strip chip, once for its pane header — so resolving
/// it at each call site repeated that walk. Carried alongside `doc` rather
/// than as another parameter so the recursive render keeps its arity.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DockContext<'a> {
    pub(crate) doc: &'a Document,
    pub(crate) theme: &'a Theme,
    pub(crate) viewer_labels: &'a HashMap<NodeId, String>,
}

/// The dock's persistent GUI state — just the drag in flight; the
/// arrangement itself lives on the `Document`.
#[derive(Debug, Default)]
pub(crate) struct DockUi {
    /// Armed by [`Self::scan`] off a chip's latched drag,
    /// resolved there into a [`DockOp::MoveTab`] on release (or
    /// cancelled by Esc), painted by [`Self::render`].
    tab_drag: Option<TabDrag>,
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
    pub(crate) fn scan(&mut self, ui: &mut Ui, doc: &Document, actions: &mut Vec<UiAction>) {
        // Ahead of the chip pass: a read-only focus query, and one that only
        // ever moves `focused`, so it composes with an activation from the
        // same scan rather than racing it.
        scan_focus(ui, doc, actions);
        for tab in doc.layout.all_tabs() {
            if strip::closable(tab) && ui.response_for(strip::tab_close_wid(tab)).left.clicked() {
                actions.push(UiAction::Dock(DockOp::CloseTab { tab }));
                continue;
            }
            if ui.response_for(strip::tab_chip_wid(tab)).left.clicked() {
                actions.push(UiAction::Dock(DockOp::ActivateTab { tab }));
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
                    text: tab_text(doc, tab).into_owned(),
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
                actions.push(UiAction::Dock(DockOp::MoveTab {
                    tab,
                    to: target.drop,
                }));
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
        &self,
        ui: &mut Ui,
        cx: DockContext<'_>,
        out: &mut Intents,
        mut content: impl FnMut(&mut Ui, TabRef, &mut Intents),
    ) {
        render_node(ui, cx, DockLayout::ROOT, DockPath::ROOT, out, &mut content);
        if let Some(dragged) = &self.tab_drag {
            ui.set_cursor(CursorIcon::Grabbing);
            draw_drag_feedback(ui, cx, dragged);
        }
    }
}

/// Recursive walk of the dock tree: a split renders as an palantir
/// `Splitter` (ratio changes surface as `DockOp::SetRatio`), a
/// group as its strip + the active tab's view.
fn render_node<F: FnMut(&mut Ui, TabRef, &mut Intents)>(
    ui: &mut Ui,
    cx: DockContext<'_>,
    idx: NodeIdx,
    path: DockPath,
    out: &mut Intents,
    content: &mut F,
) {
    match cx.doc.layout.node(idx) {
        DockNode::Group(group) => render_group(ui, cx, group, out, content),
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
                    render_node(ui, cx, child, child_path, out, content);
                });
            // The widget wrote the divider drag into `live_ratio`; the
            // layout itself only changes through the recorded intent
            // (drained post-record, coalescing per divider). Approximate
            // compare for the same reason `pan_zoom::emit_pan_zoom` uses
            // one — an exact `!=` emits on sub-epsilon jitter.
            if !live_ratio.approximately_eq(ratio) {
                out.push_dock(DockOp::SetRatio {
                    split: path,
                    ratio: live_ratio,
                });
            }
        }
    }
}

/// One pane: the group's tab strip over its active tab's view.
fn render_group<F: FnMut(&mut Ui, TabRef, &mut Intents)>(
    ui: &mut Ui,
    cx: DockContext<'_>,
    group: &TabGroup,
    out: &mut Intents,
    content: &mut F,
) {
    let labels = tab_labels(ui, cx, group);
    Panel::vstack()
        .id(pane_wid(group.id))
        .size((Sizing::FILL, Sizing::FILL))
        // Focusable so a press anywhere in the pane that misses every inner
        // focusable lands here — what [`scan_focus`] reads to move dock
        // focus. Never a keyboard target in its own right: keys route by
        // input scope, and a pane declares none.
        .focusable(true)
        .show(ui, |ui| {
            strip::show(ui, cx.theme, group, &labels, out);
            content(ui, group.active_tab(), out);
        });
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
/// Surfaces [`UiAction::FocusPane`], not a recorded `DockOp` — see
/// [`DockLayout::focus`] for why this one mutation stays out of the undo
/// record.
fn scan_focus(ui: &Ui, doc: &Document, actions: &mut Vec<UiAction>) {
    if let Some(group) = doc
        .layout
        .groups()
        .find(|g| g.id != doc.layout.focused && ui.focus_within(pane_wid(g.id)))
    {
        actions.push(UiAction::FocusPane(group.id));
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
    if let Some(target) = drop_target(ui, cx.doc) {
        let r = target.highlight;
        ui.layer(Layer::Tooltip, r.min, Some(r.size), |ui| {
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
        let label_style = sized_text(ui, 13.0);
        ui.layer(Layer::Tooltip, p + Vec2::new(14.0, 18.0), None, |ui| {
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

/// Project one group's tabs into the strip's per-tab labels — the label
/// text is the one thing the strip needs the `Document` for.
fn tab_labels(ui: &mut Ui, cx: DockContext<'_>, group: &TabGroup) -> Vec<TabLabel> {
    let focused = cx.doc.layout.focused == group.id;
    group
        .tabs
        .iter()
        .enumerate()
        .map(|(i, &tab)| TabLabel {
            tab,
            text: ui.intern(viewer_aware_text(cx, tab)),
            active: i == group.active,
            focused,
        })
        .collect()
}

/// Per-frame label for a strip chip. Viewer tabs read the caller's
/// resolved map instead of re-running [`viewer::node_label`]'s
/// recursive node search; every other kind is cheap and formats inline.
fn viewer_aware_text<'a>(cx: DockContext<'a>, tab: TabRef) -> Cow<'a, str> {
    match tab {
        // A hit skips `node_label`'s recursive search; a miss falls through
        // to it rather than to a placeholder, so the cache can only make the
        // label cheaper, never different.
        TabRef::ImageViewer(node_id) => match cx.viewer_labels.get(&node_id) {
            Some(label) => Cow::Borrowed(label.as_str()),
            None => tab_text(cx.doc, tab),
        },
        _ => tab_text(cx.doc, tab),
    }
}

/// A tab's display text — shared by the strip labels and the drag's
/// ghost chip.
fn tab_text(doc: &Document, tab: TabRef) -> Cow<'_, str> {
    match tab {
        TabRef::Graph => Cow::Borrowed("main"),
        TabRef::Preferences => Cow::Borrowed("preferences"),
        TabRef::ImageViewer(node_id) => viewer::node_label(doc, node_id).into(),
    }
}
