//! Inline rename for a subgraph boundary port's name. A plain label that
//! swaps to a `TextEdit` on double-click (via the shared
//! [`crate::gui::node::inline_rename`] widget); commit emits an
//! [`Intent::RenameBoundaryPort`]. Used only by the boundary
//! (`SubgraphInput`/`SubgraphOutput`) port rows in
//! [`crate::gui::node::port_row`]; ordinary node ports render plain text.

use palantir::{Configure, HAlign, InternedStr, Sense, Text, Tooltip, Ui, WidgetId};

use crate::core::document::BoundarySide;
use crate::core::edit::intent::Intent;
use crate::gui::PortRef;
use crate::gui::node::{RecordCtx, select_intent};
use crate::gui::widgets::inline_rename::InlineRename;

/// Character cap for a boundary-port name in the inline rename editor.
const PORT_NAME_MAX_CHARS: usize = 24;

/// Stable id for a port's rename editor — and for the sensing label
/// panel shown when idle, so the same `WidgetId` is recorded every frame
/// across the label⇄editor swap (palantir drops state rows for ids it
/// doesn't see).
fn port_rename_wid(port: PortRef) -> WidgetId {
    WidgetId::from_hash((
        "graph.node.port_rename",
        port.node_id,
        port.kind as u8,
        port.port_idx,
    ))
}

/// A port label. When `rename` is `Some`, double-clicking swaps the
/// label for a length-capped `TextEdit`; Enter or focus loss commits a
/// [`Intent::RenameBoundaryPort`], Esc cancels. `None` (regular node
/// ports and the trailing "+" placeholder) renders plain text. `tip`
/// (the port's data type) shows as a hover tooltip; empty = no tooltip.
pub(crate) fn port_label(
    ui: &mut Ui,
    rcx: RecordCtx<'_>,
    port: PortRef,
    name: InternedStr,
    tip: &str,
    rename: Option<BoundarySide>,
    out: &mut Vec<Intent>,
) {
    let Some(side) = rename else {
        // Regular node port: a plain label that opts into `Sense::HOVER`
        // (it captures no clicks, so node selection/drag still fall
        // through) so the type tooltip has a trigger anchor.
        let snapshot = Text::new(name).sense(Sense::HOVER).show(ui).snapshot();
        if !tip.is_empty() {
            Tooltip::for_(&snapshot).text(tip.to_owned()).show(ui);
        }
        return;
    };
    let shift = ui.modifiers().shift;
    let id = port_rename_wid(port);
    // Boundary inputs render in the right (output) column — text hugs
    // the right edge so it stays flush with the port circle. Boundary
    // outputs render in the left column and hug the left edge.
    let halign = match side {
        BoundarySide::Input => HAlign::Right,
        BoundarySide::Output => HAlign::Left,
    };
    let ev = InlineRename::new(id, name)
        .theme(&rcx.theme.inline_rename)
        .max_chars(PORT_NAME_MAX_CHARS)
        .halign(halign)
        .show(ui);
    // Single click selects the node (the label otherwise swallows the
    // click the body would have gotten); a committed value renames.
    if ev.clicked {
        out.push(select_intent(shift, rcx.scene, port.node_id));
    }
    if let Some(to) = ev.committed {
        out.push(Intent::RenameBoundaryPort {
            side,
            idx: port.port_idx,
            to,
        });
    }
}
