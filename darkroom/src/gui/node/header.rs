//! Node header bar: the title plus the right-aligned indicator chips
//! (`S` subgraph-open, `T` terminal, `C` cache-toggle). Drawn as the
//! top child of each node body by [`crate::gui::node::NodeUI`].

use palantir::{
    Align, Background, Color, Configure, Corners, Panel, Sense, Sizing, Spacing, Stroke, Text,
    TextStyle, Ui, VAlign, WidgetId,
};
use scenarium::prelude::{NodeBehavior, NodeId};

use crate::edit::intent::Intent;
use crate::gui::node::RecordCtx;
use crate::scene::SceneNode;
use crate::theme::Theme;

/// Side of a header indicator chip (px), and its glyph font size.
const BADGE_SIZE: f32 = 15.0;
const BADGE_FONT: f32 = 10.0;

pub(super) fn header(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let theme = rcx.theme;
    let r = theme.header_corner_radius;
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
            Text::new(node.name.clone()).show(ui);
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
                );
            }
            if node.terminal {
                badge(ui, theme, "badge_t", "T", theme.badge_terminal, true, None);
            }
            let toggled = badge(
                ui,
                theme,
                "badge_c",
                "C",
                theme.badge_cache,
                node.cached,
                Some(cache_badge_wid(node.id)),
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
        });
}

/// Stable id for a node's clickable cache-toggle chip. Domain-derived
/// so `response_for` works without threading state.
fn cache_badge_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.cache_badge", node_id))
}

/// Stable id for a subgraph node's clickable open-in-tab chip.
pub(super) fn subgraph_badge_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.subgraph_badge", node_id))
}

/// One header indicator chip: a small rounded square with a centered
/// glyph. `filled` paints a solid swatch (active/descriptor); otherwise
/// it's a hollow outline (inactive toggle). A `wid` makes it clickable
/// and the returned bool reports a click this frame; decorative chips
/// pass `None` and ignore the result.
fn badge(
    ui: &mut Ui,
    theme: &Theme,
    salt: &'static str,
    glyph: &'static str,
    color: Color,
    filled: bool,
    wid: Option<WidgetId>,
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
    panel = match wid {
        Some(w) => panel.id(w).sense(Sense::CLICK),
        None => panel.id_salt(salt),
    };
    panel
        .show(ui, |ui| {
            Text::new(glyph)
                .style(TextStyle {
                    color: glyph_color,
                    font_size_px: BADGE_FONT,
                    ..ui.theme.text
                })
                .show(ui);
        })
        .response
        .clicked()
}
