//! A port's name label: plain text with the port's data type as a hover
//! tooltip. Sensing but click-through, so node selection and drag still reach
//! the body underneath.

use palantir::{Configure, InternedStr, Sense, Text, TextStyle, Tooltip, Ui};

use crate::gui::node::RecordCtx;

/// Render `name` as the port's label. `tip` (the port's data type) shows as a
/// hover tooltip; empty means no tooltip.
///
/// Opts into [`Sense::HOVER`] rather than capturing clicks: the label needs a
/// trigger anchor for the tooltip, but the node body below it owns selection
/// and drag. Muted ink — the value column is each row's strong element, not
/// the label.
pub(super) fn port_label(ui: &mut Ui, rcx: RecordCtx<'_>, name: InternedStr, tip: &str) {
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
}
