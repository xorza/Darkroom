//! The graph-pane tab strip. Renders one chip per open graph (the
//! root + any opened subgraph interiors) and highlights the active one.
//! Draw-only: clicks are read back in `emit_tab_actions` during prepass
//! (from last frame's chip responses) so a tab switch applies *before*
//! the record — letting the target graph record a pass earlier and its
//! connections draw with no first-frame gap. Pure view state; never
//! touches the document.

use palantir::{
    Align, Background, Configure, Corners, InternedStr, Panel, Sense, Sizing, Spacing, Text,
    TextStyle, Ui, VAlign, WidgetId,
};
use scenarium::prelude::SubgraphId;

use crate::core::document::{GraphRef, TabRef};
use crate::core::edit::intent::Intent;
use crate::gui::UiAction;
use crate::gui::theme::Theme;
use crate::gui::widgets::inline_rename::InlineRename;

/// Character cap for a subgraph name in the inline rename editor.
const SUBGRAPH_NAME_MAX_CHARS: usize = 32;

/// One tab's display state, built by `main_window` from the open-tab list.
/// `subgraph_id.is_some()` marks an inline-renamable subgraph tab;
/// `closable` marks a tab that carries a close button (every tab except
/// the `Main` graph — subgraph *and* non-graph views like Config).
pub(crate) struct TabLabel {
    pub(crate) text: InternedStr,
    pub(crate) subgraph_id: Option<SubgraphId>,
    pub(crate) closable: bool,
}

/// Stable id for the tab chip at `index` — deterministic so prepass can
/// read its click without the live response.
fn tab_chip_wid(index: usize) -> WidgetId {
    WidgetId::from_hash(("graph.tab", index))
}

/// Stable id for the close button of the tab at `index`.
fn tab_close_wid(index: usize) -> WidgetId {
    WidgetId::from_hash(("graph.tab_close", index))
}

/// Stable id for the trailing "+" new-subgraph chip.
fn tab_new_wid() -> WidgetId {
    WidgetId::from_hash("graph.tab_new")
}

/// Stable id for the rename editor on a subgraph tab. Keyed on the
/// subgraph id (not tab index) so the editing state survives a tab
/// reorder.
fn tab_rename_wid(sub_id: SubgraphId) -> WidgetId {
    WidgetId::from_hash(("graph.tab_rename", sub_id))
}

/// Prepass scan: surface tab activate/close + the "+" new-subgraph
/// request from last frame's chip responses. Reads three click sources
/// per subgraph tab (close button > rename-label > outer chip), since a
/// click landing on the inner rename label is captured there and the
/// outer chip's response stays `clicked: false` — without polling the
/// label widget id here too, the activation would only fire once the
/// click happened on the chip's padding. Close wins over activate.
///
/// Reading the rename label's click in the *prepass* (not as a record-
/// pass `Intent` push) is load-bearing: it lets the navigation phase
/// settle the new target *before* this frame's record, so Pass A
/// records the new tab and Pass A's cascades feed Pass B's
/// `PortFrame` cache — without that, the switch lands in the
/// post-record drain, Pass A draws the old tab, and Pass B redraws
/// the new tab with an empty port cache (no connections that frame).
pub(crate) fn emit_tab_actions(ui: &Ui, tabs: &[TabRef], actions: &mut Vec<UiAction>) {
    for (index, tab) in tabs.iter().enumerate() {
        // Every tab except the `Main` graph carries a close button.
        let closable = !matches!(tab, TabRef::Graph(GraphRef::Main));
        if closable && ui.response_for(tab_close_wid(index)).clicked {
            actions.push(UiAction::CloseTab(index));
            continue;
        }
        let label_clicked = matches!(
            tab,
            TabRef::Graph(GraphRef::Local(id)) if ui.response_for(tab_rename_wid(*id)).clicked
        );
        if label_clicked || ui.response_for(tab_chip_wid(index)).clicked {
            actions.push(UiAction::ActivateTab(index));
        }
    }
    if ui.response_for(tab_new_wid()).clicked {
        actions.push(UiAction::NewSubgraph);
    }
}

/// Draw the strip. Tab activate / close clicks are handled in
/// [`emit_tab_actions`] (prepass); subgraph-rename commits push directly
/// into `out` from the label's inline-rename editor.
pub(crate) fn show(
    ui: &mut Ui,
    theme: &Theme,
    tabs: &[TabLabel],
    active: usize,
    out: &mut Vec<Intent>,
) {
    // The strip shares the menu bar's `chrome_fill`; the active tab
    // below punches through to `canvas_bg` so it reads as one piece with
    // the graph.
    Panel::hstack()
        .id_salt("tab_bar")
        .size((Sizing::FILL, Sizing::Hug))
        .padding(Spacing::new(6.0, 4.0, 6.0, 0.0))
        .gap(3.0)
        .child_align(Align::v(VAlign::Bottom))
        .background(Background {
            fill: theme.chrome_fill.into(),
            ..Default::default()
        })
        .show(ui, |ui| {
            for (i, tab) in tabs.iter().enumerate() {
                tab_chip(ui, theme, tab, i, i == active, out);
            }
            new_tab_chip(ui, theme);
        });
}

/// The trailing "+" chip that creates and opens a fresh subgraph. Reads
/// as an inactive tab (flat on the chrome strip); the click is consumed
/// in [`emit_tab_actions`].
fn new_tab_chip(ui: &mut Ui, theme: &Theme) {
    let hover_bg = if ui.response_for(tab_new_wid()).hovered {
        Background {
            fill: theme.header_fill.into(),
            corners: Corners::all(3.0),
            ..Default::default()
        }
    } else {
        Background::default()
    };
    Panel::zstack()
        .id(tab_new_wid())
        .size((Sizing::Fixed(20.0), Sizing::Hug))
        .sense(Sense::CLICK)
        .padding(Spacing::xy(0.0, 4.0))
        .child_align(Align::CENTER)
        .background(hover_bg)
        .show(ui, |ui| {
            Text::new("+")
                .style(TextStyle {
                    color: theme.text_muted,
                    font_size_px: 15.0,
                    line_height_mult: 1.0,
                    ..ui.theme.text
                })
                .text_align(Align::CENTER)
                .show(ui);
        });
}

fn tab_chip(
    ui: &mut Ui,
    theme: &Theme,
    tab: &TabLabel,
    index: usize,
    active: bool,
    out: &mut Vec<Intent>,
) {
    let r = theme.tab_corner_radius;
    // Active tab takes the graph's color so its bottom edge dissolves
    // into the canvas; inactive tabs stay flat on the chrome strip
    // (transparent fill = just a label) so the active one reads clearly.
    let background = if active {
        Background {
            fill: theme.canvas_bg.into(),
            corners: Corners::new(r, r, 0.0, 0.0),
            ..Default::default()
        }
    } else {
        Background::default()
    };
    // A closable tab trades right inset for the top-right close button
    // (equal 4px top/right gaps); Main stays symmetric so its label is
    // centered.
    let padding = if tab.closable {
        Spacing::new(10.0, 4.0, 4.0, 4.0)
    } else {
        Spacing::xy(10.0, 4.0)
    };
    // Match the menu bar's smaller (13px) label scale on every tab.
    let label_style = TextStyle {
        font_size_px: 13.0,
        ..ui.theme.text
    };
    Panel::hstack()
        .id(tab_chip_wid(index))
        .size((Sizing::Hug, Sizing::Hug))
        .sense(Sense::CLICK)
        .padding(padding)
        .gap(6.0)
        .child_align(Align::v(VAlign::Center))
        .background(background)
        .show(ui, |ui| {
            // Subgraph tab: inline-renamable label. Double-click swaps to
            // a `TextEdit`; Enter / blur commits. A single click on the
            // label also switches tab (the label's own panel captures it,
            // so the outer chip's click handler in `emit_tab_actions`
            // wouldn't see it).
            if let Some(sub_id) = tab.subgraph_id {
                // `clicked` is *not* forwarded to a `SwitchTab` intent
                // here — `emit_tab_actions` polls the same response in
                // the prepass and pushes the activation as a
                // `UiAction`, so the switch settles before this
                // frame's record. Push-on-click during record would
                // defer the switch to the post-record drain, landing
                // the new tab in Pass B with no measured layouts and
                // dropping its connections for a frame.
                let ev = InlineRename::new(tab_rename_wid(sub_id), tab.text.clone())
                    .theme(&theme.inline_rename)
                    .max_chars(SUBGRAPH_NAME_MAX_CHARS)
                    .style(label_style)
                    .show(ui);
                if let Some(to) = ev.committed {
                    out.push(Intent::RenameSubgraph { id: sub_id, to });
                }
            } else {
                // Main / non-graph tab: plain label, activation handled by
                // the outer chip in `emit_tab_actions`.
                Text::new(tab.text.clone()).style(label_style).show(ui);
            }
            if tab.closable {
                let close_wid = tab_close_wid(index);
                // Hover comes from last frame's response; paint a subtle
                // highlight chip behind the `×` when pointed at.
                let hover_bg = if ui.response_for(close_wid).hovered {
                    Background {
                        fill: theme.header_fill.into(),
                        corners: Corners::all(3.0),
                        ..Default::default()
                    }
                } else {
                    Background::default()
                };
                Panel::zstack()
                    .id(close_wid)
                    .size((Sizing::Fixed(16.0), Sizing::Fixed(16.0)))
                    .sense(Sense::CLICK)
                    // Pin to the chip's top edge so it reads as a
                    // top-right corner close (it's already the rightmost
                    // item in the row).
                    .align(Align::v(VAlign::Top))
                    .child_align(Align::CENTER)
                    .background(hover_bg)
                    .show(ui, |ui| {
                        // `×` at a size that fits the 16px box (the
                        // default 16px font overflows and rides high) and
                        // in a legible muted-light ink, not the near-bg
                        // border color it was before.
                        Text::new("\u{00d7}")
                            .style(TextStyle {
                                color: theme.text_muted,
                                font_size_px: 13.0,
                                // Hug the glyph (no 1.2× leading) so the
                                // line box ≈ the glyph and it centers in
                                // the 16px button instead of riding high.
                                line_height_mult: 1.0,
                                ..ui.theme.text
                            })
                            .text_align(Align::CENTER)
                            .show(ui);
                    });
            }
        });
}
