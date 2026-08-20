//! The canvas's keyboard chords: Esc-deselect, Ctrl+0 reset-zoom, Ctrl+D
//! duplicate, and Delete/Backspace.
//!
//! Raised from [`GraphUI::prepass`](crate::gui::pane::graph::GraphUI::prepass) beside the pointer
//! gestures, because they are the same kind of thing — this frame's input
//! turned into intents, drained before the record. Being reached from the
//! canvas is also what scopes them: a chord names no pane of its own, so
//! running only while a graph pane is up is what keeps Delete from removing
//! nodes nobody is looking at.

use palantir::{Key, Shortcut, Ui};

use crate::core::document::Viewport;
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::requests::Requests;

const RESET_ZOOM_SHORTCUT: Shortcut = Shortcut::ctrl('0');
const DUPLICATE_SHORTCUT: Shortcut = Shortcut::ctrl('D');

/// Turn this frame's canvas chords into intents.
///
/// Routed through the intent stack (not a direct doc write) so they land in
/// the undo history; the `is_noop` filter in `drain_requests` drops them when
/// they'd change nothing. Every chord is sampled up front rather than
/// short-circuited, so one firing doesn't drop the others' subscription for
/// palantir's wake gate. The `Edit`- and `Escape`-class ones stand down on
/// their own while a text field holds focus, and Ctrl+0 / Ctrl+D are `Accel`
/// so they keep firing mid-edit.
pub(crate) fn emit(ui: &mut Ui, graph_ctx: GraphCtx<'_>, out: &mut Requests) {
    let reset_zoom = ui.key_pressed(RESET_ZOOM_SHORTCUT);
    let escape = ui.escape_pressed();
    let duplicate = ui.key_pressed(DUPLICATE_SHORTCUT);
    let delete =
        ui.key_pressed(Shortcut::key(Key::Delete)) || ui.key_pressed(Shortcut::key(Key::Backspace));
    let view = graph_ctx.view();
    if escape && !view.selected.is_empty() {
        out.push_graph(GraphIntent::clear_selection());
    }
    if reset_zoom {
        out.push_graph(GraphIntent::SetViewport {
            to: Viewport {
                pan: view.viewport.pan,
                zoom: 1.0,
            },
        });
    }
    // Ctrl+D drops wires from producers outside the selection; keeping them
    // is the node menu's second Duplicate pick, which has a label to say so.
    if duplicate {
        out.extend_graph(GraphIntent::duplicate(graph_ctx.document(), false));
    }
    if delete {
        out.extend_graph(
            view.selected
                .iter()
                .map(|&node_id| GraphIntent::RemoveNode { node_id }),
        );
    }
}
