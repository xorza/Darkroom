//! The canvas's context before its geometry settles: the graph plus what this
//! frame's input means on it, which bare-canvas gesture a press latches among them, the one
//! place that precedence is decided.

use palantir::{Modifiers, PointerButton, ResponseState};

use crate::gui::graph_ctx::GraphCtx;
use crate::gui::port_ref::PortRef;

/// What this frame's input means on the canvas, resolved once at the top of
/// [`GraphUi::prepass`](crate::gui::pane::graph_ui::GraphUi::prepass) and read
/// back by every phase after it: which bare-canvas gesture latched, what
/// Esc does, and the wire a release dropped on empty canvas.
///
/// Read there rather than per controller because each is one fact about the
/// frame, and controllers polling them separately is how they grew different
/// guards around the same key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CanvasInput {
    pub(crate) gesture: Option<CanvasGesture>,
    pub(crate) escape: Escape,
    /// The source of a held wire released over empty canvas this frame, which opens the
    /// new-node palette; the wire floats from it again once a node is picked.
    pub(crate) dropped_wire: Option<PortRef>,
}

/// Which bare-canvas gesture a fresh press or click latches this frame. Resolved once by
/// [`CanvasGesture::classify`], so each controller under
/// [`gesture`](crate::gui::pane::graph_ui::gesture) consumes only its own variant and none keeps
/// itself disjoint from the others by hand.
///
/// Covers the *latch* frame only: continuation of an in-flight gesture is tracked by each
/// controller's own state, and wheel/pinch zoom coexists with everything (handled in
/// `CameraGesture::prepass`, not here).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CanvasGesture {
    /// Middle-button drag → viewport pan.
    Pan,
    /// Plain LMB-drag (no modifier) → rubber-band selection.
    Select,
    /// Ctrl+LMB-drag or RMB-drag → wire breaker. Carries the button that latched it, since the
    /// breaker polls that same button for continuation and release.
    Breaker(PointerButton),
    /// The breaker's chord plus Shift → tie line: Shift+RMB-drag, or Ctrl+Shift+LMB-drag on a
    /// trackpad. `darkroom/AGENTS.md`, "Wire ties", "The tie line is Shift+RMB-drag", holds the
    /// decision. Carries the button, as the breaker does.
    Tie(PointerButton),
    /// RMB-click or LMB double-click on empty canvas (no drag) → new-node popup.
    NewNode,
    /// LMB-click (no drag) → clear selection.
    Deselect,
}

/// Whether the right press now held, or released this frame, already ended a floating wire, so
/// its click opens nothing more.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SecondaryPress {
    #[default]
    Free,
    Spent,
}

impl CanvasGesture {
    /// The bare-canvas gesture of the outer canvas's response `resp` with `modifiers` held.
    /// Drag-starts are checked before clicks; `None` when nothing latched — an idle canvas, or a
    /// press a node or port captured, as a node widget captures its own press and the canvas
    /// sees only presses that missed every one.
    pub(crate) const fn classify(
        resp: &ResponseState,
        modifiers: Modifiers,
        secondary: SecondaryPress,
    ) -> Option<Self> {
        if resp.middle.drag.started() {
            return Some(Self::Pan);
        }
        if resp.right.drag.started() {
            return Some(if modifiers.shift {
                Self::Tie(PointerButton::Right)
            } else {
                Self::Breaker(PointerButton::Right)
            });
        }
        if resp.left.drag.started() {
            return Some(match (modifiers.ctrl, modifiers.shift) {
                (true, true) => Self::Tie(PointerButton::Left),
                (true, false) => Self::Breaker(PointerButton::Left),
                // Shift extends the band selection; the band reads it.
                (false, _) => Self::Select,
            });
        }
        // A double-click sets `clicked` and `double_clicked` on one frame, so it precedes the
        // plain click's `Deselect`; the first click of the pair already cleared the selection.
        let right_click = resp.right.clicked() && matches!(secondary, SecondaryPress::Free);
        if right_click || resp.left.double_clicked() {
            return Some(Self::NewNode);
        }
        if resp.clicked() {
            return Some(Self::Deselect);
        }
        None
    }
}

/// What this frame's Esc does on the canvas. A pressed Esc cancels whatever
/// gesture is in flight; with nothing in flight as the frame starts, it also
/// clears the selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Escape {
    #[default]
    Unpressed,
    /// Pressed while a gesture, a menu or the palette was in flight.
    Cancels,
    /// Pressed with nothing in flight.
    Deselects,
}

/// The graph canvas before its geometry is settled: the graph, plus what this
/// frame's input means on it.
///
/// The prepass level of the context chain, between the [`GraphCtx`] and the
/// [`CanvasCtx`](crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx), which adds the geometry. The
/// two prepass steps that run *before*
/// [`CanvasGeometry::rebuild`](crate::gui::pane::graph_ui::frame::canvas_geometry::CanvasGeometry::rebuild)
/// — pan/zoom and the node-drag advance — take this, so they read the frame's input without a
/// geometry someone is still writing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PrepassCtx<'a> {
    graph_ctx: GraphCtx<'a>,
    input: CanvasInput,
}

impl<'a> PrepassCtx<'a> {
    pub(super) const fn new(graph_ctx: GraphCtx<'a>, input: CanvasInput) -> Self {
        Self { graph_ctx, input }
    }

    pub(crate) const fn graph_ctx(self) -> GraphCtx<'a> {
        self.graph_ctx
    }

    pub(crate) const fn input(self) -> CanvasInput {
        self.input
    }

    /// Whether this frame's Esc cancels whatever gesture is in flight.
    pub(crate) const fn cancelled(self) -> bool {
        !matches!(self.input.escape, Escape::Unpressed)
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;
    use palantir::{ButtonPhase, ButtonState, Drag, Modifiers, PointerButton, ResponseState};

    use crate::gui::pane::graph_ui::prepass_ctx::{CanvasGesture, SecondaryPress};

    /// Every arm, each from the one button edge that latches it: a drag start beats a click, the
    /// modifiers pick the drag's gesture, a double click opens the palette over the plain click's
    /// deselect, and a right click spent ending a wire opens nothing.
    #[test]
    fn each_press_latches_its_gesture_by_precedence() {
        let dragged = ButtonState {
            phase: ButtonPhase::Held,
            drag: Drag::Started { delta: Vec2::X },
        };
        let clicked = |count| ButtonState {
            phase: ButtonPhase::Up { click: Some(count) },
            drag: Drag::None,
        };
        let mods = |ctrl, shift| Modifiers {
            ctrl,
            shift,
            ..Modifiers::default()
        };
        let none = Modifiers::default();
        let (left, right) = (PointerButton::Left, PointerButton::Right);
        let left_by = |state| ResponseState {
            left: state,
            ..ResponseState::default()
        };
        let right_by = |state| ResponseState {
            right: state,
            ..ResponseState::default()
        };
        let middle_by = |state| ResponseState {
            middle: state,
            ..ResponseState::default()
        };
        let both = ResponseState {
            left: clicked(1),
            middle: dragged,
            ..ResponseState::default()
        };
        let free = SecondaryPress::Free;
        let cases = [
            ("idle", ResponseState::default(), none, free, None),
            (
                "middle drag",
                middle_by(dragged),
                none,
                free,
                Some(CanvasGesture::Pan),
            ),
            (
                "a drag beats a click",
                both,
                none,
                free,
                Some(CanvasGesture::Pan),
            ),
            (
                "right drag",
                right_by(dragged),
                none,
                free,
                Some(CanvasGesture::Breaker(right)),
            ),
            (
                "shift right drag",
                right_by(dragged),
                mods(false, true),
                free,
                Some(CanvasGesture::Tie(right)),
            ),
            (
                "left drag",
                left_by(dragged),
                none,
                free,
                Some(CanvasGesture::Select),
            ),
            (
                "shift left drag",
                left_by(dragged),
                mods(false, true),
                free,
                Some(CanvasGesture::Select),
            ),
            (
                "ctrl left drag",
                left_by(dragged),
                mods(true, false),
                free,
                Some(CanvasGesture::Breaker(left)),
            ),
            (
                "ctrl shift left drag",
                left_by(dragged),
                mods(true, true),
                free,
                Some(CanvasGesture::Tie(left)),
            ),
            (
                "right click",
                right_by(clicked(1)),
                none,
                free,
                Some(CanvasGesture::NewNode),
            ),
            (
                "spent right click",
                right_by(clicked(1)),
                none,
                SecondaryPress::Spent,
                None,
            ),
            (
                "double click",
                left_by(clicked(2)),
                none,
                free,
                Some(CanvasGesture::NewNode),
            ),
            (
                "left click",
                left_by(clicked(1)),
                none,
                free,
                Some(CanvasGesture::Deselect),
            ),
            (
                "spent leaves the left",
                left_by(clicked(1)),
                none,
                SecondaryPress::Spent,
                Some(CanvasGesture::Deselect),
            ),
        ];
        for (what, resp, modifiers, secondary, gesture) in cases {
            assert_eq!(
                CanvasGesture::classify(&resp, modifiers, secondary),
                gesture,
                "{what}"
            );
        }
    }
}
