//! Which gesture a press on *bare canvas* latches — the one place that
//! precedence is decided.
//!
//! Every controller under [`gesture`](super) consumes only its own variant of
//! [`CanvasGesture`], so none of them has to keep itself disjoint from the
//! others by hand. `GraphUI::prepass` resolves the frame's answer once and
//! parks it; the controllers read it back out of the canvas context.

use palantir::{PointerButton, Ui};

use crate::gui::pane::graph::canvas::outer_canvas_widget_id;

/// Whether the modifier reserving an output-port drag for spawning a preview
/// is held. The one place that chord is decided: [`PreviewDrag`](super::preview_drag::PreviewDrag) claims an
/// output drag under it, and `ConnectionUI` drops the output column from its
/// latch candidates under the same condition — stated once so the two cannot
/// drift into both claiming, or neither.
pub(crate) fn preview_drag_modifier(ui: &mut Ui) -> bool {
    ui.modifiers().ctrl
}

/// Which bare-canvas gesture a fresh press/click latches this frame.
/// Resolved once by [`classify_canvas_gesture`] so the precedence among
/// the competing controllers lives in a single place rather than being
/// re-derived (and kept disjoint by hand) in each one.
///
/// Covers the *latch* frame only: continuation of an in-flight gesture is
/// tracked by each controller's own `Option<state>`, and wheel/pinch zoom
/// coexists with everything (handled in `emit_pan_zoom`, not here).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CanvasGesture {
    /// Middle-button drag → viewport pan.
    Pan,
    /// Plain LMB-drag (no modifier) → rubber-band selection.
    Select,
    /// Ctrl+LMB-drag or RMB-drag → connection breaker. Carries the button
    /// that latched it, since the breaker polls that same button for
    /// continuation/release (a Ctrl+LMB breaker must keep reading Left).
    Breaker(PointerButton),
    /// RMB-click or LMB double-click on empty canvas (no drag) → new-node
    /// popup.
    NewNode,
    /// LMB-click (no drag) → clear selection.
    Deselect,
}

/// Resolve `target`'s bare-canvas gesture for this frame from that pane's
/// outer-canvas response + modifiers. Drag-starts are checked before clicks
/// (palantir reports `clicked`/`secondary_clicked` only on a release that
/// *didn't* drag, but the explicit ordering keeps the precedence obvious).
/// `None` when nothing latched — an idle canvas, or a press a node/port
/// captured. With several panes open at most one can answer `Some`: the
/// press lands on exactly one canvas.
///
/// This only ever sees presses that *missed* every node and port: a
/// node/badge widget captures its own press, so a right-click on a node
/// body routes to `node_menu` (which reads
/// those widgets' `secondary_clicked` directly) and never reaches here —
/// `NewNode` is therefore right-click-on-*empty*-canvas by construction.
pub(crate) fn classify_canvas_gesture(ui: &mut Ui) -> Option<CanvasGesture> {
    let resp = ui.response_for(outer_canvas_widget_id());
    if resp.middle.drag.started() {
        return Some(CanvasGesture::Pan);
    }
    if resp.right.drag.started() {
        return Some(CanvasGesture::Breaker(PointerButton::Right));
    }
    if resp.left.drag.started() {
        return Some(if ui.modifiers().ctrl {
            CanvasGesture::Breaker(PointerButton::Left)
        } else {
            CanvasGesture::Select
        });
    }
    // A double-click sets `clicked` *and* `double_click` on the same frame,
    // so this must precede the plain-click `Deselect` arm to win. The first
    // click of the pair already ran its own `Deselect`, so the selection is
    // clear by the time the popup opens.
    if resp.right.clicked() || resp.left.double_clicked() {
        return Some(CanvasGesture::NewNode);
    }
    if resp.left.clicked() {
        return Some(CanvasGesture::Deselect);
    }
    None
}
