//! Node header bar: the title plus the right-aligned indicator chips
//! (`S` subgraph-open, `T` terminal, `C` cache-toggle). Drawn as the
//! top child of each node body by [`crate::gui::node::NodeUI`].

use palantir::{
    Align, Background, Color, Configure, Corners, Panel, Sense, Sizing, Spacing, Stroke, Text,
    TextStyle, Tooltip, Ui, VAlign, WidgetId,
};
use scenarium::prelude::{NodeBehavior, NodeId};

use crate::core::edit::intent::Intent;
use crate::gui::canvas::inspector::{InspectMode, inspect_badge_wid};
use crate::gui::node::{RecordCtx, exec_color, node_rename_wid, select_intent};
use crate::gui::run_state::ExecStatus;
use crate::gui::scene::SceneNode;
use crate::gui::theme::Theme;
use crate::gui::widgets::inline_rename::InlineRename;

/// Character cap for a node title in the inline rename editor.
const NODE_NAME_MAX_CHARS: usize = 32;

/// Side of a header indicator chip (px), and its glyph font size.
const BADGE_SIZE: f32 = 15.0;
const BADGE_FONT: f32 = 10.0;

/// Compact run-time label: seconds → `s` / `ms` / `µs` at the scale
/// that keeps 2–3 significant digits.
pub(crate) fn fmt_elapsed(secs: f64) -> String {
    if secs >= 1.0 {
        format!("{secs:.2}s")
    } else if secs >= 1e-3 {
        format!("{:.1}ms", secs * 1e3)
    } else {
        format!("{:.0}µs", secs * 1e6)
    }
}

/// The header bar: just the node title. Indicator chips + the run-time
/// label live in [`status_row`] below it so adding/removing the time
/// label never reflows the title.
pub(crate) fn header(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let theme = rcx.theme;
    // The header sits inside the body's border stroke (the layout folds
    // the stroke width into the body's padding). Its top corners must
    // follow the stroke's *inner* radius — `node_corner_radius` minus the
    // stroke width (the node draws a `2 × node_border_width` stroke) —
    // otherwise the header's rounder corner leaves a wedge of body
    // `node_fill` showing between it and the (selection-lit) stroke.
    let r = (theme.node_corner_radius - theme.node_border_width * 2.0).max(0.0);
    Panel::hstack()
        .id_salt("header")
        .size((Sizing::FILL, Sizing::Hug))
        .padding(Spacing::xy(8.0, 4.0))
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .background(Background {
            fill: theme.header_fill.into(),
            corners: Corners::new(r, r, 0.0, 0.0),
            ..Default::default()
        })
        .show(ui, |ui| {
            title(ui, rcx, node, out);
            // FILL spacer pushes the inspect chip to the header's right
            // edge, opposite the title.
            Panel::hstack()
                .id_salt("header_spacer")
                .size((Sizing::FILL, Sizing::Hug))
                .show(ui, |_| {});
            // Inspect toggle: filled (checked) when pinned, accent outline
            // when open, muted-grey outline (`text_muted`) when closed —
            // visible on the header without competing with the accent
            // S/T/C badges. The click is consumed in `Inspectors::apply`
            // via this chip's deterministic id, so the returned flag is
            // ignored here. Hidden on boundary nodes — they're pure
            // routing, no runtime values / status worth inspecting.
            if !node.boundary {
                let mode = rcx.inspectors.mode(node.id);
                let color = if mode.is_some() {
                    theme.badge_subgraph
                } else {
                    theme.text_muted
                };
                badge(
                    ui,
                    theme,
                    "badge_inspect",
                    "i",
                    color,
                    mode == Some(InspectMode::Pinned),
                    Some(inspect_badge_wid(node.id)),
                    "Inspect — values, status, log",
                );
            }
        });
}

/// The status strip under the header: the last-run time label (left) and
/// the indicator chips (`S` subgraph-open, `T` terminal, `C` cache),
/// right-aligned. Its own row so the time appearing/disappearing doesn't
/// resize the header; the cache chip always shows, so the row's height is
/// reserved regardless.
pub(crate) fn status_row(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let theme = rcx.theme;
    Panel::hstack()
        .id_salt("status_row")
        .size((Sizing::FILL, Sizing::Hug))
        .padding(Spacing::xy(8.0, 2.0))
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            // Last-run time, in the node's status color so it ties to the
            // glow. Only executed nodes carry a time.
            if let ExecStatus::Executed(secs) = node.exec_status {
                let color = exec_color(theme, node.exec_status).unwrap_or(ui.theme.text.color);
                Text::new(fmt_elapsed(secs))
                    .style(TextStyle {
                        color,
                        font_size_px: BADGE_FONT,
                        ..ui.theme.text
                    })
                    .show(ui);
            }
            // FILL spacer pushes the badge cluster to the right edge.
            Panel::hstack()
                .id_salt("badge_spacer")
                .size((Sizing::FILL, Sizing::Hug))
                .show(ui, |_| {});
            // Subgraph chip is the open-in-tab affordance. We only *draw*
            // it here (with its stable id); the click is read next frame
            // in `emit_subgraph_opens` (prepass) so the open applies
            // before the record — letting the subgraph record a pass
            // earlier and its connections draw with no first-frame gap.
            if node.subgraph.is_some() {
                badge(
                    ui,
                    theme,
                    "badge_sg",
                    "S",
                    theme.badge_subgraph,
                    true,
                    Some(subgraph_badge_wid(node.id)),
                    "Open subgraph",
                );
            }
            if node.terminal {
                badge(
                    ui,
                    theme,
                    "badge_t",
                    "T",
                    theme.badge_terminal,
                    true,
                    None,
                    "Terminal — output sink",
                );
            }
            let toggled = badge(
                ui,
                theme,
                "badge_c",
                "C",
                theme.badge_cache,
                node.cached,
                Some(cache_badge_wid(node.id)),
                "Compute once (cache the result)",
            );
            if toggled {
                out.push(Intent::SetCacheBehavior {
                    node_id: node.id,
                    to: if node.cached {
                        NodeBehavior::AsFunction
                    } else {
                        NodeBehavior::Once
                    },
                });
            }
            // Disable toggle: filled when the node is excluded from
            // execution. Muted swatch (it's "off", not an alarm).
            let disable_toggled = badge(
                ui,
                theme,
                "badge_d",
                "D",
                theme.text_muted,
                node.disabled,
                Some(disable_badge_wid(node.id)),
                "Disable — exclude from the run",
            );
            if disable_toggled {
                out.push(Intent::SetDisabled {
                    node_id: node.id,
                    to: !node.disabled,
                });
            }
        });
}

/// The node title: an inline-renamable label. Double-click swaps it for
/// a `TextEdit`; commit emits [`Intent::RenameNode`], single-click
/// selects (the label would otherwise swallow the body's click). Same
/// widget + style as the boundary-port rename in
/// [`crate::gui::node::port_rename`].
fn title(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let shift = ui.modifiers().shift;
    let id = node_rename_wid(node.id);
    let ev = InlineRename::new(id, node.name.clone())
        .theme(&rcx.theme.inline_rename)
        .max_chars(NODE_NAME_MAX_CHARS)
        .show(ui);
    if ev.clicked {
        out.push(select_intent(shift, rcx.scene, node.id));
    }
    if let Some(to) = ev.committed {
        out.push(Intent::RenameNode {
            node_id: node.id,
            to,
        });
    }
}

/// Stable id for a node's clickable cache-toggle chip. Domain-derived
/// so `response_for` works without threading state.
fn cache_badge_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.cache_badge", node_id))
}

/// Stable id for a node's clickable enable/disable chip.
fn disable_badge_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.disable_badge", node_id))
}

/// Stable id for a subgraph node's clickable open-in-tab chip.
pub(crate) fn subgraph_badge_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.subgraph_badge", node_id))
}

/// One header indicator chip: a small rounded square with a centered
/// glyph. `filled` paints a solid swatch (active/descriptor); otherwise
/// it's a hollow outline (inactive toggle). A `wid` makes it clickable
/// and the returned bool reports a click this frame; decorative chips
/// pass `None` and ignore the result.
#[allow(clippy::too_many_arguments)]
fn badge(
    ui: &mut Ui,
    theme: &Theme,
    salt: &'static str,
    glyph: &'static str,
    color: Color,
    filled: bool,
    wid: Option<WidgetId>,
    tip: &str,
) -> bool {
    let background = if filled {
        Background {
            fill: color.into(),
            corners: Corners::all(3.0),
            ..Default::default()
        }
    } else {
        Background {
            stroke: Stroke::solid(color, 1.0),
            corners: Corners::all(3.0),
            ..Default::default()
        }
    };
    // Solid chips carry dark glyphs (contrast against the swatch);
    // hollow chips ink the glyph in the accent itself.
    let glyph_color = if filled { theme.header_fill } else { color };
    let mut panel = Panel::zstack()
        .size((Sizing::Fixed(BADGE_SIZE), Sizing::Fixed(BADGE_SIZE)))
        .child_align(Align::CENTER)
        .background(background);
    // Clickable chips capture the press; decorative ones (`T`) still opt
    // into `HOVER` so their tooltip fires without swallowing the click.
    panel = match wid {
        Some(w) => panel.id(w).sense(Sense::CLICK),
        None => panel.id_salt(salt).sense(Sense::HOVER),
    };
    let chip = panel.show(ui, |ui| {
        Text::new(glyph)
            .style(TextStyle {
                color: glyph_color,
                font_size_px: BADGE_FONT,
                ..ui.theme.text
            })
            .show(ui);
    });
    // Take the owned snapshot + click result so the chip's `ui` borrow ends
    // before the tooltip records into `ui`.
    let snapshot = chip.response.snapshot();
    let clicked = chip.response.clicked();
    if !tip.is_empty() {
        Tooltip::for_(&snapshot).text(tip.to_owned()).show(ui);
    }
    clicked
}
