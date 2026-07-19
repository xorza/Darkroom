//! Inline rename for a graph boundary port's name. A plain label that
//! swaps to a `TextEdit` on double-click (via the shared
//! [`crate::gui::node::inline_rename`] widget); commit emits an
//! [`Intent::RenameBoundaryPort`]. Used only by the boundary
//! (`GraphInput`/`GraphOutput`) port rows in
//! [`crate::gui::node::port_row`]; ordinary node ports render plain text.

use aperture::{Configure, HAlign, InternedStr, Sense, Text, TextStyle, Tooltip, Ui, WidgetId};

use crate::core::document::BoundarySide;
use crate::core::document::ItemRef;
use crate::core::document::PortRef;
use crate::core::edit::intent::types::Intent;
use crate::gui::node::{RecordCtx, click_intents};
use crate::gui::widgets::inline_rename::InlineRename;

/// Character cap for a boundary-port name in the inline rename editor.
const PORT_NAME_MAX_CHARS: usize = 24;

/// Stable id for a port's rename editor — and for the sensing label
/// panel shown when idle, so the same `WidgetId` is recorded every frame
/// across the label⇄editor swap (aperture drops state rows for ids it
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
        // through) so the type tooltip has a trigger anchor. Muted ink —
        // the value column is each row's strong element, not the label.
        let snapshot = Text::new(name)
            .style(&TextStyle {
                color: rcx.theme.colors.port_label,
                ..ui.theme.text.clone()
            })
            .sense(Sense::HOVER)
            .show(ui)
            .snapshot();
        if !tip.is_empty() {
            Tooltip::on(&snapshot).text(tip).show(ui);
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
    let ev = InlineRename::new(id, name, &rcx.theme.inline_rename)
        .max_chars(PORT_NAME_MAX_CHARS)
        .halign(halign)
        .show(ui);
    // Single click selects the node (the label otherwise swallows the
    // click the body would have gotten); a committed value renames.
    if ev.clicked {
        click_intents(shift, rcx.scene, ItemRef::Node(port.node_id), out);
    }
    if let Some(to) = ev.committed {
        out.push(Intent::RenameBoundaryPort {
            side,
            idx: port.port_idx,
            to,
        });
    }
}
