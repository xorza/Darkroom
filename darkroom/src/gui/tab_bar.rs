//! A pane's tab strip. Renders one chip per open tab in a [`TabGroup`]
//! and highlights the active one (accent cap when the group is focused).
//! Draw-only: clicks are read back in `emit_tab_actions` during prepass
//! (from last frame's chip responses) so a tab switch applies *before*
//! the record — letting the target view record a pass earlier and its
//! connections draw with no first-frame gap. The right-click split menu
//! is the one record-time emitter here (its picks are this-frame values,
//! pushed as `Intent`s directly). Pure view state; never touches the
//! document.

use aperture::{
    Align, Background, Configure, ContextMenu, Corners, MenuItem, Panel, Sense, Sizing, SmolStr,
    Spacing, Text, TextStyle, Ui, VAlign, WidgetId,
};
use scenarium::graph::subgraph::SubgraphId;

use crate::core::document::dock::{DockDrop, DockLayout, SplitSide, TabGroup, TabGroupId};
use crate::core::document::{GraphRef, TabRef};
use crate::core::edit::intent::{DockIntent, Intent};
use crate::gui::UiAction;
use crate::gui::theme::Theme;
use crate::gui::widgets::inline_rename::InlineRename;

/// Character cap for a subgraph name in the inline rename editor.
const SUBGRAPH_NAME_MAX_CHARS: usize = 32;

/// One tab's display state, built by `main_window` from its group: the
/// tab plus its resolved label text (the one projection that needs the
/// `Document`). Everything else a chip renders — closability,
/// movability, renamability — is derived from the tab itself.
pub(crate) struct TabLabel {
    pub(crate) tab: TabRef,
    pub(crate) text: SmolStr,
}

/// Every tab except the pinned `Main` graph carries a close button.
fn closable(tab: TabRef) -> bool {
    tab != TabRef::Graph(GraphRef::Main)
}

/// Non-graph tabs can move between panes (drag or the split menu);
/// graph tabs are pinned to the primary pane.
pub(crate) fn movable(tab: TabRef) -> bool {
    !matches!(tab, TabRef::Graph(_))
}

/// The subgraph behind an inline-renamable tab (a `Local` graph tab).
fn renamable_subgraph(tab: TabRef) -> Option<SubgraphId> {
    match tab {
        TabRef::Graph(GraphRef::Local(id)) => Some(id),
        _ => None,
    }
}

/// Stable id for `group`'s tab chip at `index` — deterministic so the
/// prepass (activation clicks, the drag scan) can read it without the
/// live response.
pub(crate) fn tab_chip_wid(group: TabGroupId, index: usize) -> WidgetId {
    WidgetId::from_hash(("dock.tab", group, index))
}

/// Stable id for `group`'s whole strip row — the drag scan's
/// insertion-zone rect.
pub(crate) fn strip_wid(group: TabGroupId) -> WidgetId {
    WidgetId::from_hash(("dock.strip", group))
}

/// Stable id for the close button of `group`'s tab at `index`.
fn tab_close_wid(group: TabGroupId, index: usize) -> WidgetId {
    WidgetId::from_hash(("dock.tab_close", group, index))
}

/// Stable id for the split context menu of `group`'s tab at `index`.
fn tab_menu_wid(group: TabGroupId, index: usize) -> WidgetId {
    WidgetId::from_hash(("dock.tab_menu", group, index))
}

/// Stable id for the rename editor on a subgraph tab. Keyed on the
/// subgraph id (not group/index) so the editing state survives the tab
/// moving or reordering.
fn tab_rename_wid(sub_id: SubgraphId) -> WidgetId {
    WidgetId::from_hash(("dock.tab_rename", sub_id))
}

/// Stable id for the trailing "+" new-subgraph chip (currently hidden —
/// see [`new_tab_chip`]).
fn tab_new_wid() -> WidgetId {
    WidgetId::from_hash("dock.tab_new")
}

/// Prepass scan over every group's strip: surface tab activate/close
/// requests from last frame's chip responses. Reads three click sources
/// per subgraph tab (close button > rename-label > outer chip), since a
/// click landing on the inner rename label is captured there and the
/// outer chip's response stays `clicked: false`. Close wins over
/// activate.
///
/// Reading these in the *prepass* (not as record-pass pushes) is
/// load-bearing: it lets the navigation phase settle the new target
/// *before* this frame's record, so Pass A records the new view and its
/// cascades feed Pass B's `CanvasGeometry` cache.
pub(crate) fn emit_tab_actions(ui: &Ui, layout: &DockLayout, actions: &mut Vec<UiAction>) {
    for group in layout.groups() {
        for (index, &tab) in group.tabs.iter().enumerate() {
            if closable(tab) && ui.response_for(tab_close_wid(group.id, index)).clicked {
                actions.push(UiAction::CloseTab {
                    group: group.id,
                    index,
                });
                continue;
            }
            let label_clicked = renamable_subgraph(tab)
                .is_some_and(|id| ui.response_for(tab_rename_wid(id)).clicked);
            if label_clicked || ui.response_for(tab_chip_wid(group.id, index)).clicked {
                actions.push(UiAction::ActivateTab {
                    group: group.id,
                    index,
                });
            }
        }
    }
    if ui.response_for(tab_new_wid()).clicked {
        actions.push(UiAction::NewSubgraph);
    }
}

/// Side of the square "+" chip. Matches the tab chips' content height —
/// the 13px label's 1.2× line box plus their 4px top/bottom insets — so it
/// stands exactly as tall as a tab while being square.
const NEW_TAB_CHIP_SIDE: f32 = 13.0 * 1.2 + 8.0;

/// The trailing "+" chip that creates and opens a fresh subgraph. A square,
/// tab-shaped chip (top corners rounded like the tabs, bottom square) that
/// reads as an inactive tab; the click is consumed in [`emit_tab_actions`].
///
/// Hidden from the strip for now — kept intact (and its click still
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

/// One strip's shared draw state, threaded through its chips (the
/// [`crate::gui::node::RecordCtx`] pattern, strip-scoped).
struct StripCtx<'a> {
    theme: &'a Theme,
    group: TabGroupId,
    /// Whether this strip's group holds the dock focus — the accent cap
    /// dims elsewhere so one pane always reads as "where actions go".
    focused: bool,
    out: &'a mut Vec<Intent>,
}

/// Draw one group's strip. Tab activate / close clicks are handled in
/// [`emit_tab_actions`] (prepass); subgraph-rename commits and split-menu
/// picks push directly into `out` this frame.
pub(crate) fn show(
    ui: &mut Ui,
    theme: &Theme,
    group: &TabGroup,
    labels: &[TabLabel],
    focused: bool,
    out: &mut Vec<Intent>,
) {
    let mut strip = StripCtx {
        theme,
        group: group.id,
        focused,
        out,
    };
    // The strip wears the chrome band; the active tab below punches
    // through to `canvas_bg` so it reads as one piece with the pane.
    Panel::hstack()
        .id(strip_wid(group.id))
        .size((Sizing::FILL, Sizing::Hug))
        .padding(Spacing::new(6.0, 4.0, 6.0, 0.0))
        .gap(3.0)
        .child_align(Align::v(VAlign::Bottom))
        .background(Background {
            fill: theme.colors.chrome_fill.into(),
            ..Default::default()
        })
        .show(ui, |ui| {
            for (i, label) in labels.iter().enumerate() {
                tab_chip(ui, &mut strip, label, i, i == group.active);
            }
        });
}

fn tab_chip(ui: &mut Ui, s: &mut StripCtx<'_>, label: &TabLabel, index: usize, active: bool) {
    let theme = s.theme;
    let r = theme.tab_corner_radius;
    // Active-tab selection cue: a 2px accent cap along the top, built from two
    // layered backgrounds. The outer is filled with the accent and rounded to
    // the full `r`; the inner tab fill is nested `ACCENT` px lower with a
    // tighter corner, so the accent peeks out only as a thin top band that
    // follows the rounded corners. The cap wears the full accent only in the
    // focused group. Inactive tabs skip the cap and wear a faint
    // `tab_inactive` chip; the active fill stays `canvas_bg` so its bottom
    // dissolves into the pane.
    const ACCENT: f32 = 2.0;
    let outer_top = if active { ACCENT } else { 0.0 };
    let outer_bg = if active {
        let cap = if s.focused {
            theme.colors.selection_rect
        } else {
            theme.colors.header_fill
        };
        Background {
            fill: cap.into(),
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
    let padding = if closable(label.tab) {
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
    // accent shows only as a top cap. Movable tabs also sense drags —
    // the docking gesture (`gui::dock_drag`); the 4 px latch threshold
    // keeps plain clicks working unchanged.
    let sense = if movable(label.tab) {
        Sense::CLICK | Sense::DRAG
    } else {
        Sense::CLICK
    };
    Panel::hstack()
        .id(tab_chip_wid(s.group, index))
        .size((Sizing::Hug, Sizing::Hug))
        .sense(sense)
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
                    if let Some(sub_id) = renamable_subgraph(label.tab) {
                        // `clicked` is *not* forwarded to an activation intent
                        // here — `emit_tab_actions` polls the same response in
                        // the prepass and pushes the activation as a
                        // `UiAction`, so the switch settles before this frame's
                        // record. Push-on-click during record would defer the
                        // switch to the post-record drain, landing the new tab
                        // in Pass B with no measured layouts and dropping its
                        // connections for a frame.
                        let ev = InlineRename::new(tab_rename_wid(sub_id), label.text.clone())
                            .theme(&theme.inline_rename)
                            .max_chars(SUBGRAPH_NAME_MAX_CHARS)
                            .style(label_style)
                            .show(ui);
                        if let Some(to) = ev.committed {
                            s.out.push(Intent::RenameSubgraph { id: sub_id, to });
                        }
                    } else {
                        // Main / non-graph tab: plain label, activation handled
                        // by the outer chip in `emit_tab_actions`.
                        Text::new(label.text.clone()).style(label_style).show(ui);
                    }
                    if closable(label.tab) {
                        close_button(ui, theme, tab_close_wid(s.group, index));
                    }
                });
        });

    if movable(label.tab) {
        split_menu(ui, s, label.tab, index);
    }
}

/// The chip's top-right `×`. Hover comes from last frame's response; the
/// click is consumed in [`emit_tab_actions`].
fn close_button(ui: &mut Ui, theme: &Theme, close_wid: WidgetId) {
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
        // Pin to the chip's top edge so it reads as a top-right corner
        // close (it's already the rightmost item in the row).
        .align(Align::v(VAlign::Top))
        .child_align(Align::CENTER)
        .background(hover_bg)
        .show(ui, |ui| {
            // `×` at a size that fits the 16px box (the default 16px font
            // overflows and rides high) and in a legible muted-light ink.
            Text::new("\u{00d7}")
                .style(TextStyle {
                    color: theme.colors.text_muted,
                    font_size_px: 13.0,
                    // Hug the glyph (no 1.2× leading) so the line box ≈ the
                    // glyph and it centers in the 16px button instead of
                    // riding high.
                    line_height_mult: 1.0,
                    ..ui.theme.text
                })
                .text_align(Align::CENTER)
                .show(ui);
        });
}

/// Right-click split menu — the phase-1 stand-in for drag-docking (and a
/// keeper: cheap and discoverable). Opens on the chip's secondary click;
/// a pick moves `tab` into a fresh pane on the chosen side.
fn split_menu(ui: &mut Ui, s: &mut StripCtx<'_>, tab: TabRef, index: usize) {
    let menu_wid = tab_menu_wid(s.group, index);
    if ui
        .response_for(tab_chip_wid(s.group, index))
        .secondary_clicked
        && let Some(p) = ui.pointer_pos()
    {
        ContextMenu::open(ui, menu_wid, p);
    }
    ContextMenu::for_id(menu_wid)
        .size((Sizing::Hug, Sizing::Hug))
        .show(ui, |ui, popup| {
            let mut side = None;
            if MenuItem::new("Split right").show(ui, popup).clicked() {
                side = Some(SplitSide::Right);
            }
            if MenuItem::new("Split down").show(ui, popup).clicked() {
                side = Some(SplitSide::Bottom);
            }
            if let Some(side) = side {
                s.out.push(Intent::Dock(DockIntent::MoveTab {
                    tab,
                    to: DockDrop::Split {
                        group: s.group,
                        side,
                    },
                }));
            }
        });
}
