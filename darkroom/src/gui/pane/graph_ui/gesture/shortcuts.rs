//! The canvas's keyboard chords: Esc-deselect, Ctrl+0 reset-zoom, Ctrl+D
//! duplicate, and Delete/Backspace.
//!
//! Raised from [`GraphUi::prepass`](crate::gui::pane::graph_ui::GraphUi::prepass) beside the
//! pointer gestures, because they are the same kind of thing — this frame's input turned into
//! intents, drained before the record. Being reached from the canvas is also what scopes them: a
//! chord names no pane of its own, so running only while a graph pane is up is what keeps Delete
//! from removing nodes nobody is looking at.

use palantir::{Key, Ui};
use scenarium::NodeId;

use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::incoming_wires::IncomingWires;
use crate::gui::frame::requests::Requests;
use crate::gui::frame::shortcuts;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::gesture::pan_zoom::Framing;

/// Turn this frame's canvas chords into intents.
///
/// Routed through the intent stack (not a direct document write) so they land in
/// the undo history; the `is_noop` filter in `drain_requests` drops them when
/// they'd change nothing. Every chord is sampled up front rather than
/// short-circuited, so one firing doesn't drop the others' subscription for
/// palantir's wake gate. The `Edit`-class ones stand down on their own while a
/// text field holds focus, and Ctrl+0 / Ctrl+D are `Accel` so they keep firing
/// mid-edit.
///
/// Esc deselects only when the canvas `ctx` says nothing was in flight: only
/// the canvas knows whether the Esc cancelled a gesture instead.
pub(crate) fn emit(ui: &mut Ui, ctx: CanvasCtx<'_>, out: &mut Requests) {
    let graph_ctx = ctx.graph_ctx();
    let reset_zoom = ui.shortcut_pressed(shortcuts::RESET_VIEW);
    let duplicate = ui.shortcut_pressed(shortcuts::DUPLICATE);
    let delete = ui.shortcut_pressed(shortcuts::DELETE) || ui.shortcut_pressed(Key::Backspace);
    let view = graph_ctx.view();
    if ctx.deselects() && !view.selected.is_empty() {
        out.push_graph(GraphIntent::clear_selection());
    }
    // Ctrl+0 is the toolbar's reset view: 1:1, centered on the content.
    if reset_zoom && let Some(reset) = Framing::Reset.intent(ui, ctx) {
        out.push_graph(reset);
    }
    // Ctrl+D drops wires from producers outside the selection; keeping them
    // is the node menu's second Duplicate pick, which has a label to say so.
    if duplicate {
        out.extend_graph(GraphIntent::duplicate(
            graph_ctx.document(),
            graph_ctx.library(),
            IncomingWires::Drop,
            || NodeId::unique().as_u128(),
        ));
    }
    if delete {
        out.extend_graph(GraphIntent::remove_selection(view));
    }
}

#[cfg(test)]
mod tests {
    use palantir::{Key, Modifiers};

    use crate::core::document::internals::DocumentFixture;
    use crate::core::document::item_id::ItemId;
    use crate::core::edit::graph_intent::GraphIntent;
    use crate::gui::pane::graph_ui::internals::CanvasHarness;

    /// Over a selected node at rest: Delete and Backspace remove it, Esc clears the selection,
    /// Ctrl+0 resets the view and Ctrl+D duplicates the node; each chord raises only its own.
    #[test]
    fn each_chord_raises_its_intent() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        let plain = Modifiers::default();
        let cases = [
            (Key::Delete, plain, "remove"),
            (Key::Backspace, plain, "remove"),
            (Key::Escape, plain, "deselect"),
            (Key::Char('0'), ctrl, "reset view"),
            (Key::Char('d'), ctrl, "duplicate"),
        ];
        for (key, mods, want) in cases {
            let mut h = CanvasHarness::new(DocumentFixture::probes(1));
            let node = h.node(0);
            h.document_mut().main_view.selected = [ItemId::Node(node)].into_iter().collect();
            // Off the default view, so the reset has a view to move.
            h.document_mut().main_view.viewport.zoom = 2.0;
            h.prime(2);
            h.ui.set_modifiers(mods);
            h.ui.key(key);
            let intents = h.frame();
            let raised = match &*intents {
                [GraphIntent::RemoveNode { node_id }] if *node_id == node => "remove",
                [GraphIntent::SetSelection { to }] if to.is_empty() => "deselect",
                [GraphIntent::SetViewport { to, .. }] if to.zoom == 1.0 => "reset view",
                [GraphIntent::AddNode { .. }, ..] => "duplicate",
                other => panic!("{key:?}: {other:?}"),
            };
            assert_eq!(raised, want, "{key:?}");
        }
    }
}
