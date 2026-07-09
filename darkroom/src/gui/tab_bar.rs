//! The graph-pane tab strip. Renders one chip per open graph (the
//! root + any opened subgraph interiors) and highlights the active one.
//! Draw-only: clicks are read back in `emit_tab_actions` during prepass
//! (from last frame's chip responses) so a tab switch applies *before*
//! the record — letting the target graph record a pass earlier and its
//! connections draw with no first-frame gap. Pure view state; never
//! touches the document.

use aperture::{
    Align, Background, Configure, Corners, Panel, Sense, Sizing, SmolStr, Spacing, Text, TextStyle,
    Ui, VAlign, WidgetId,
};
use scenarium::graph::subgraph::SubgraphId;

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
/// the `Main` graph — subgraph *and* non-graph views like Preferences).
pub(crate) struct TabLabel {
    pub(crate) text: SmolStr,
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
/// `CanvasGeometry` cache — without that, the switch lands in the
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
            fill: theme.colors.chrome_fill.into(),
            ..Default::default()
        })
        .show(ui, |ui| {
            for (i, tab) in tabs.iter().enumerate() {
                tab_chip(ui, theme, tab, i, i == active, out);
            }
        });
}

/// Side of the square "+" chip. Matches the tab chips' content height —
/// the 13px label's 1.2× line box plus their 4px top/bottom insets — so it
/// stands exactly as tall as a tab while being square.
const NEW_TAB_CHIP_SIDE: f32 = 13.0 * 1.2 + 8.0;

/// The trailing "+" chip that creates and opens a fresh subgraph. A square,
/// tab-shaped chip (top corners rounded like the tabs, bottom square) that
/// reads as an inactive tab; the click is consumed in [`emit_tab_actions`].
///
/// Hidden from the tab strip for now — kept intact (and its click still
/// wired in `emit_tab_actions`) so it can be re-enabled without rebuilding it.
#[allow(dead_code)]
fn new_tab_chip(ui: &mut Ui, theme: &Theme) {
    let r = theme.tab_corner_radius;
    let hover_bg = if ui.response_for(tab_new_wid()).hovered {
        Background {
            fill: theme.colors.header_fill.into(),
            corners: Corners::new(r, r, 0.0, 0.0),
            ..Default::default()
        }
    } else {
        Background::default()
    };
    Panel::zstack()
        .id(tab_new_wid())
        .size((
            Sizing::Fixed(NEW_TAB_CHIP_SIDE),
            Sizing::Fixed(NEW_TAB_CHIP_SIDE),
        ))
        .sense(Sense::CLICK)
        .child_align(Align::CENTER)
        .background(hover_bg)
        .show(ui, |ui| {
            Text::new("+")
                .style(TextStyle {
                    color: theme.colors.text_muted,
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
    // Active-tab selection cue: a 2px accent cap along the top, built from two
    // layered backgrounds. The outer is filled with the accent and rounded to
    // the full `r`; the inner tab fill is nested `ACCENT` px lower with a
    // tighter corner, so the accent peeks out only as a thin top band that
    // follows the rounded corners (an inset background shadow can't do this —
    // the encoder paints the fill *over* it). Inactive tabs skip the accent
    // and wear a faint `tab_inactive` chip so the strip reads as tabs; the
    // active fill stays `canvas_bg` so its bottom still dissolves into the
    // graph.
    const ACCENT: f32 = 2.0;
    let outer_top = if active { ACCENT } else { 0.0 };
    let outer_bg = if active {
        Background {
            fill: theme.colors.selection_rect.into(),
            corners: Corners::new(r, r, 0.0, 0.0),
            ..Default::default()
        }
    } else {
        Background::default()
    };
    let inner_r = if active { (r - ACCENT).max(0.0) } else { r };
    let inner_fill = if active {
        theme.colors.canvas_bg
    } else {
        theme.colors.tab_inactive
    };
    let inner_bg = Background {
        fill: inner_fill.into(),
        corners: Corners::new(inner_r, inner_r, 0.0, 0.0),
        ..Default::default()
    };
    // A closable tab trades right inset for the top-right close button (equal
    // 4px top/right gaps); Main stays symmetric so its label is centered. The
    // active tab lifts its inner top inset by `ACCENT`, so the cap adds no
    // height and the label sits at the same place on every tab.
    let inner_top = 4.0 - outer_top;
    let padding = if tab.closable {
        Spacing::new(10.0, inner_top, 4.0, 4.0)
    } else {
        Spacing::new(10.0, inner_top, 10.0, 4.0)
    };
    // Match the menu bar's smaller (13px) label scale on every tab; the
    // active tab carries full-strength ink, inactive tabs recede to muted.
    let label_style = TextStyle {
        font_size_px: 13.0,
        color: if active {
            ui.theme.text.color
        } else {
            theme.colors.text_muted
        },
        ..ui.theme.text
    };
    // Outer carries the accent fill + click sense + the 2px top inset; the
    // inner carries the tab fill + content, nested `ACCENT` px lower so the
    // accent shows only as a top cap.
    Panel::hstack()
        .id(tab_chip_wid(index))
        .size((Sizing::Hug, Sizing::Hug))
        .sense(Sense::CLICK)
        .padding(Spacing::new(0.0, outer_top, 0.0, 0.0))
        .background(outer_bg)
        .show(ui, |ui| {
            Panel::hstack()
                .id_salt("tab_content")
                .size((Sizing::Hug, Sizing::Hug))
                .padding(padding)
                .gap(6.0)
                .child_align(Align::v(VAlign::Center))
                .background(inner_bg)
                .show(ui, |ui| {
                    // Subgraph tab: inline-renamable label. Double-click swaps
                    // to a `TextEdit`; Enter / blur commits. A single click on
                    // the label also switches tab (the label's own panel
                    // captures it, so the outer chip's click handler in
                    // `emit_tab_actions` wouldn't see it).
                    if let Some(sub_id) = tab.subgraph_id {
                        // `clicked` is *not* forwarded to a `SwitchTab` intent
                        // here — `emit_tab_actions` polls the same response in
                        // the prepass and pushes the activation as a
                        // `UiAction`, so the switch settles before this frame's
                        // record. Push-on-click during record would defer the
                        // switch to the post-record drain, landing the new tab
                        // in Pass B with no measured layouts and dropping its
                        // connections for a frame.
                        let ev = InlineRename::new(tab_rename_wid(sub_id), tab.text.clone())
                            .theme(&theme.inline_rename)
                            .max_chars(SUBGRAPH_NAME_MAX_CHARS)
                            .style(label_style)
                            .show(ui);
                        if let Some(to) = ev.committed {
                            out.push(Intent::RenameSubgraph { id: sub_id, to });
                        }
                    } else {
                        // Main / non-graph tab: plain label, activation handled
                        // by the outer chip in `emit_tab_actions`.
                        Text::new(tab.text.clone()).style(label_style).show(ui);
                    }
                    if tab.closable {
                        let close_wid = tab_close_wid(index);
                        // Hover comes from last frame's response; paint a subtle
                        // highlight chip behind the `×` when pointed at.
                        let hover_bg = if ui.response_for(close_wid).hovered {
                            Background {
                                fill: theme.colors.header_fill.into(),
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
                                // default 16px font overflows and rides high)
                                // and in a legible muted-light ink, not the
                                // near-bg border color it was before.
                                Text::new("\u{00d7}")
                                    .style(TextStyle {
                                        color: theme.colors.text_muted,
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
        });
}
