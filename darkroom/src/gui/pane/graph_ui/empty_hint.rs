//! What an empty graph says at its center: how to add its first node.

use glam::Vec2;
use palantir::{Align, Configure, Panel, Sizing, Text, Ui, WidgetId, fmt};
use scenarium::GraphId;

use crate::gui::command::Command;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::theme::Theme;
use crate::gui::widgets::support;

/// One muted line at the center of `graph`'s pane, `Shift+A or a double-click adds a node`, in
/// place of a bare grid. It takes no input, so a double-click on it lands on the canvas it names.
/// Nothing records on the first frame, before the pane has a size to center in.
pub(super) fn show(ui: &mut Ui, graph: GraphId, theme: &Theme) {
    let Some(pane) = ui.response_for(canvas::outer_wid(graph)).layout_rect else {
        return;
    };
    let keys = Command::AddNode
        .keys()
        .expect("Add Node has a chord, which the reference names");
    let style = support::muted_text(ui, theme, theme.font.body);
    Panel::zstack()
        .id(wid(graph))
        .at(Vec2::ZERO)
        .size((Sizing::fixed(pane.size.w), Sizing::fixed(pane.size.h)))
        .child_align(Align::CENTER)
        .show(ui, |ui| {
            Text::new(fmt!(ui, "{keys} or a double-click adds a node"))
                .style(&style)
                .show(ui);
        });
}

/// The hint's panel in `graph`'s pane.
pub(crate) fn wid(graph: GraphId) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.empty_hint", graph))
}
