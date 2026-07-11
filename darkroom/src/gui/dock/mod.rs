//! The dock's GUI half: pane tree rendering, per-group tab strips,
//! divider resizing, and tab drag-and-drop — everything between the
//! persisted model (`core::document::dock`) and the pane *content*,
//! which stays the caller's. [`DockUi`] is the whole integration
//! surface, two calls wide:
//!
//! - [`DockUi::scan`] in the navigation phase — surfaces tab
//!   activate/close clicks and drives the drag lifecycle off last
//!   frame's responses, emitting `UiAction`s.
//! - [`DockUi::render`] in the record — walks the split tree (splits as
//!   aperture `Splitter`s whose ratio drags surface as
//!   `DockOp::SetRatio`, groups as strip-over-content panes) and,
//!   mid-drag, paints the drop-zone highlight + ghost chip and holds
//!   the grabbing cursor. The `content` closure renders the active
//!   tab's view, so this module never learns what a canvas or a viewer
//!   is.
//!
//! Submodules: `strip` (the chip row) and `drag` (gesture state + the
//! pure pointer→drop-zone math).

pub(crate) mod drag;
pub(crate) mod strip;

use aperture::{
    Background, Configure, Corners, CursorIcon, Layer, Panel, Rect, Sizing, SmolStr, Spacing,
    SplitHalf, Splitter, Stroke, Text, TextStyle, Ui, WidgetId,
};
use glam::Vec2;

use crate::core::document::dock::{
    DockLayout, DockNode, DockOp, DockPath, DockSplit, NodeIdx, SplitDir, TabGroup, TabGroupId,
};
use crate::core::document::{Document, GraphRef, TabRef};
use crate::core::edit::intent::Intent;
use crate::gui::UiAction;
use crate::gui::dock::drag::{DropTarget, TabDrag, classify_drop};
use crate::gui::dock::strip::TabLabel;
use crate::gui::image_viewer;
use crate::gui::theme::Theme;

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

/// The dock's persistent GUI state — just the drag in flight; the
/// arrangement itself lives on the `Document`.
#[derive(Debug, Default)]
pub(crate) struct DockUi {
    /// Armed by [`Self::scan`] off a movable chip's latched drag,
    /// resolved there into a [`DockOp::MoveTab`] on release (or
    /// cancelled by Esc), painted by [`Self::render`].
    tab_drag: Option<TabDrag>,
}

impl DockUi {
    /// Navigation-phase scan over last frame's chip responses, one pass
    /// over every strip: close clicks (which win over activation),
    /// activation clicks (a subgraph tab's inner rename label captures
    /// the click, so its response is polled too), drag arming on a
    /// movable chip's latched drag — then the in-flight drag's
    /// lifecycle: cancel on Esc (or the tab vanishing under it), and on
    /// release resolve the pane under the pointer into a
    /// [`DockOp::MoveTab`].
    ///
    /// Scanning in the *prepass* (not as record-time pushes) is
    /// load-bearing: the navigation phase settles the new arrangement
    /// before this frame's record, so a switch — or a committed drop —
    /// draws the same frame it lands.
    pub(crate) fn scan(&mut self, ui: &mut Ui, doc: &Document, actions: &mut Vec<UiAction>) {
        for group in doc.layout.groups() {
            for (index, &tab) in group.tabs.iter().enumerate() {
                if strip::closable(tab)
                    && ui
                        .response_for(strip::tab_close_wid(group.id, index))
                        .clicked
                {
                    actions.push(UiAction::Dock(DockOp::CloseTab {
                        group: group.id,
                        index,
                    }));
                    continue;
                }
                let label_clicked = strip::renamable_subgraph(tab)
                    .is_some_and(|id| ui.response_for(strip::tab_rename_wid(id)).clicked);
                if label_clicked
                    || ui
                        .response_for(strip::tab_chip_wid(group.id, index))
                        .clicked
                {
                    actions.push(UiAction::Dock(DockOp::ActivateTab {
                        group: group.id,
                        index,
                    }));
                }
                if self.tab_drag.is_none()
                    && strip::movable(tab)
                    && ui
                        .response_for(strip::tab_chip_wid(group.id, index))
                        .drag_started()
                {
                    self.tab_drag = Some(TabDrag {
                        tab,
                        source: (group.id, index),
                        text: tab_text(doc, tab),
                    });
                }
            }
        }
        if ui.response_for(strip::tab_new_wid()).clicked {
            actions.push(UiAction::NewSubgraph);
        }

        let Some(dragged) = &self.tab_drag else {
            return;
        };
        let (tab, (src_group, src_index)) = (dragged.tab, dragged.source);
        if ui.escape_pressed() || doc.layout.find_tab(tab).is_none() {
            self.tab_drag = None;
            return;
        }
        if ui
            .response_for(strip::tab_chip_wid(src_group, src_index))
            .drag_stopped()
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
        theme: &Theme,
        doc: &Document,
        out: &mut Vec<Intent>,
        mut content: impl FnMut(&mut Ui, TabRef, &mut Vec<Intent>),
    ) {
        render_node(
            ui,
            theme,
            doc,
            DockLayout::ROOT,
            DockPath::ROOT,
            out,
            &mut content,
        );
        if let Some(dragged) = &self.tab_drag {
            ui.set_cursor(CursorIcon::Grabbing);
            draw_drag_feedback(ui, theme, doc, dragged);
        }
    }
}

/// Recursive walk of the dock tree: a split renders as an aperture
/// `Splitter` (ratio changes surface as `DockOp::SetRatio`), a
/// group as its strip + the active tab's view.
fn render_node<F: FnMut(&mut Ui, TabRef, &mut Vec<Intent>)>(
    ui: &mut Ui,
    theme: &Theme,
    doc: &Document,
    idx: NodeIdx,
    path: DockPath,
    out: &mut Vec<Intent>,
    content: &mut F,
) {
    match doc.layout.node(idx) {
        DockNode::Group(group) => render_group(ui, theme, doc, group, out, content),
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
                    render_node(ui, theme, doc, child, child_path, out, content);
                });
            // The widget wrote the divider drag into `live_ratio`; the
            // layout itself only changes through the recorded intent
            // (drained post-record, coalescing per divider).
            if live_ratio != ratio {
                out.push(Intent::Dock(DockOp::SetRatio {
                    split: path,
                    ratio: live_ratio,
                }));
            }
        }
    }
}

/// One pane: the group's tab strip over its active tab's view.
fn render_group<F: FnMut(&mut Ui, TabRef, &mut Vec<Intent>)>(
    ui: &mut Ui,
    theme: &Theme,
    doc: &Document,
    group: &TabGroup,
    out: &mut Vec<Intent>,
    content: &mut F,
) {
    let labels = tab_labels(doc, group);
    let focused = doc.layout.focused == group.id;
    Panel::vstack()
        .id(pane_wid(group.id))
        .size((Sizing::FILL, Sizing::FILL))
        .show(ui, |ui| {
            strip::show(ui, theme, group, &labels, focused, out);
            content(ui, group.active_tab(), out);
        });
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
        let chips: Vec<Rect> = (0..group.tabs.len())
            .filter_map(|i| ui.response_for(strip::tab_chip_wid(group.id, i)).rect)
            .collect();
        return Some(classify_drop(
            group.id,
            pane,
            strip_rect,
            &chips,
            doc.layout.can_split(group.id),
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
fn draw_drag_feedback(ui: &mut Ui, theme: &Theme, doc: &Document, dragged: &TabDrag) {
    let accent = theme.colors.selection_rect;
    if let Some(target) = drop_target(ui, doc) {
        let r = target.highlight;
        ui.layer(Layer::Tooltip, r.min, Some(r.size), |ui| {
            Panel::zstack()
                .id(WidgetId::from_hash("dock.drag_highlight"))
                .size((Sizing::FILL, Sizing::FILL))
                .background(Background {
                    fill: accent.with_alpha(0.18).into(),
                    stroke: Stroke {
                        brush: accent.into(),
                        width: 1.5,
                    },
                    corners: Corners::all(2.0),
                    ..Default::default()
                })
                .show(ui, |_| {});
        });
    }
    if let Some(p) = ui.pointer_pos() {
        let text = dragged.text.clone();
        let label_style = TextStyle {
            font_size_px: 13.0,
            ..ui.theme.text
        };
        ui.layer(Layer::Tooltip, p + Vec2::new(14.0, 18.0), None, |ui| {
            Panel::hstack()
                .id(WidgetId::from_hash("dock.drag_ghost"))
                .size((Sizing::Hug, Sizing::Hug))
                .padding(Spacing::new(10.0, 4.0, 10.0, 4.0))
                .background(Background {
                    fill: theme.colors.chrome_fill.into(),
                    stroke: Stroke {
                        brush: accent.into(),
                        width: 1.0,
                    },
                    corners: Corners::all(4.0),
                    ..Default::default()
                })
                .show(ui, |ui| {
                    Text::new(text).style(label_style).show(ui);
                });
        });
    }
}

/// Project one group's tabs into the strip's per-tab labels — the label
/// text is the one thing the strip needs the `Document` for.
fn tab_labels(doc: &Document, group: &TabGroup) -> Vec<TabLabel> {
    group
        .tabs
        .iter()
        .map(|&tab| TabLabel {
            tab,
            text: tab_text(doc, tab),
        })
        .collect()
}

/// A tab's display text — shared by the strip labels and the drag's
/// ghost chip.
fn tab_text(doc: &Document, tab: TabRef) -> SmolStr {
    match tab {
        TabRef::Graph(GraphRef::Main) => "main".into(),
        TabRef::Graph(GraphRef::Local(id)) => doc
            .graph
            .subgraphs
            .by_key(&id)
            .map(|d| d.name.as_str())
            .unwrap_or("subgraph")
            .into(),
        TabRef::Preferences => "preferences".into(),
        TabRef::ImageViewer(port) => image_viewer::port_label(doc, port).into(),
    }
}
