//! What this frame's input means on the canvas: which bare-canvas gesture a press latches among
//! them, the one place that precedence is decided, and what Esc does.

use palantir::{Modifiers, PointerButton, Response, ShortcutMods};

use crate::gui::widgets::format::gesture_keys::GestureKeys;

/// What this frame's input means on the canvas, resolved once at the top of
/// [`GraphUi::input`](crate::gui::pane::graph_ui::GraphUi::input) and read
/// back by every controller after it: which bare-canvas gesture latched, and what
/// Esc does. It lives on the input stage alone.
///
/// Read there rather than per controller, as controllers polling separately grew different
/// guards around the same key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CanvasInput {
    pub(crate) gesture: Option<CanvasGesture>,
    pub(crate) escape: Escape,
}

impl CanvasInput {
    /// Whether this frame's Esc cancels whatever gesture is in flight.
    pub(crate) const fn cancelled(self) -> bool {
        !matches!(self.escape, Escape::Unpressed)
    }
}

/// Which bare-canvas gesture a fresh press or click latches this frame. Resolved once by
/// [`CanvasGesture::classify`], so each controller under
/// [`gesture`](crate::gui::pane::graph_ui::gesture) consumes only its own variant and none keeps
/// itself disjoint from the others by hand.
///
/// Covers the *latch* frame only: continuation of an in-flight gesture is tracked by each
/// controller's own state, and scroll and pinch coexist with everything (handled in
/// `CameraGesture::input`, not here).
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

impl CanvasGesture {
    /// The bare-canvas gesture of the outer canvas's response `resp` with `modifiers` held.
    /// Drag-starts are checked before clicks; `None` when nothing latched — an idle canvas, or a
    /// press a node or port captured, as a node widget captures its own press and the canvas
    /// sees only presses that missed every one.
    pub(crate) const fn classify(resp: &Response, modifiers: Modifiers) -> Option<Self> {
        if resp.middle.drag.started() {
            return Some(Self::Pan);
        }
        if resp.right.drag.started() {
            return Some(if modifiers.contains(Modifiers::SHIFT) {
                Self::Tie(PointerButton::Right)
            } else {
                Self::Breaker(PointerButton::Right)
            });
        }
        if resp.left.drag.started() {
            return Some(
                match (
                    modifiers.contains(Modifiers::CTRL),
                    modifiers.contains(Modifiers::SHIFT),
                ) {
                    (true, true) => Self::Tie(PointerButton::Left),
                    (true, false) => Self::Breaker(PointerButton::Left),
                    // Shift extends the band selection; the band reads it.
                    (false, _) => Self::Select,
                },
            );
        }
        // A double-click sets `clicked` and `double_clicked` on one frame, so it precedes the
        // plain click's `Deselect`; the first click of the pair already cleared the selection.
        if resp.right.clicked() || resp.left.double_clicked() {
            return Some(Self::NewNode);
        }
        if resp.clicked() {
            return Some(Self::Deselect);
        }
        None
    }
}

/// What a bare-canvas gesture is reached by and what it does, in the words the reference and
/// the status bar show.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GestureHint {
    pub(crate) keys: GestureKeys,
    pub(crate) does: &'static str,
}

impl CanvasGesture {
    /// Every gesture the bare canvas takes, a two-button one once a button.
    pub(crate) const ALL: [Self; 8] = [
        Self::Select,
        Self::Deselect,
        Self::NewNode,
        Self::Pan,
        Self::Breaker(PointerButton::Left),
        Self::Breaker(PointerButton::Right),
        Self::Tie(PointerButton::Left),
        Self::Tie(PointerButton::Right),
    ];

    /// The gesture's keys and what it does. Beside [`Self::classify`], which it puts into words,
    /// so the two change together.
    pub(crate) const fn hint(self) -> GestureHint {
        const SHIFT: ShortcutMods = ShortcutMods::SHIFT;
        const CTRL: ShortcutMods = ShortcutMods::CTRL;
        let (mods, pointer, does) = match self {
            Self::Select => (ShortcutMods::NONE, "drag", "Select with a band"),
            Self::Deselect => (ShortcutMods::NONE, "click", "Clear the selection"),
            Self::NewNode => (ShortcutMods::NONE, "double-click", "Add a node"),
            Self::Pan => (ShortcutMods::NONE, "middle-drag", "Pan"),
            Self::Breaker(PointerButton::Right) => {
                (ShortcutMods::NONE, "right-drag", "Cut the wires crossed")
            }
            Self::Breaker(_) => (CTRL, "drag", "Cut the wires crossed"),
            Self::Tie(PointerButton::Right) => (SHIFT, "right-drag", "Tie the wires crossed"),
            Self::Tie(_) => (CTRL.union(SHIFT), "drag", "Tie the wires crossed"),
        };
        GestureHint {
            keys: GestureKeys { mods, pointer },
            does,
        }
    }
}

/// What this frame's Esc does on the canvas: it cancels whatever gesture is in flight, and
/// with nothing in flight as the frame starts, it also clears the selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Escape {
    #[default]
    Unpressed,
    /// Pressed while a gesture, a menu or the palette was in flight.
    Cancels,
    /// Pressed with nothing in flight.
    Deselects,
}

#[cfg(test)]
mod tests {
    use glam::Vec2;
    use palantir::{
        Drag, Modifiers, PointerButton, PointerButtonPhase, PointerButtonState, Response,
        ShortcutMods, WidgetId,
    };

    use crate::gui::pane::graph_ui::canvas_input::CanvasGesture;

    /// Each gesture's hint names the button and the modifiers that latch it: a drag of that
    /// button under those modifiers classifies back to the gesture.
    #[test]
    fn a_drag_gestures_hint_is_what_latches_it() {
        let dragged = PointerButtonState {
            phase: PointerButtonPhase::Held,
            drag: Drag::Started { delta: Vec2::X },
        };
        for gesture in CanvasGesture::ALL {
            let hint = gesture.hint();
            assert!(!hint.does.is_empty(), "{gesture:?}");
            let idle = Response::idle(WidgetId::from_hash("probe"));
            let resp = match hint.keys.pointer {
                "drag" => Response {
                    left: dragged,
                    ..idle
                },
                "right-drag" => Response {
                    right: dragged,
                    ..idle
                },
                "middle-drag" => Response {
                    middle: dragged,
                    ..idle
                },
                // The two clicks: `each_press_latches_its_gesture_by_precedence` holds them.
                "click" | "double-click" => continue,
                other => panic!("{gesture:?} names no pointer part: {other}"),
            };
            let mut held = Modifiers::NONE;
            held.set(Modifiers::CTRL, hint.keys.mods.contains(ShortcutMods::CTRL));
            held.set(
                Modifiers::SHIFT,
                hint.keys.mods.contains(ShortcutMods::SHIFT),
            );
            assert_eq!(CanvasGesture::classify(&resp, held), Some(gesture));
        }
    }

    /// Every arm, each from the one button edge that latches it: a drag start beats a click, the
    /// modifiers pick the drag's gesture, and a double click opens the palette over the plain
    /// click's deselect.
    #[test]
    fn each_press_latches_its_gesture_by_precedence() {
        let dragged = PointerButtonState {
            phase: PointerButtonPhase::Held,
            drag: Drag::Started { delta: Vec2::X },
        };
        let clicked = |count| PointerButtonState {
            phase: PointerButtonPhase::Up { click: Some(count) },
            drag: Drag::None,
        };
        let mods = |ctrl, shift| {
            let mut mods = Modifiers::NONE;
            mods.set(Modifiers::CTRL, ctrl);
            mods.set(Modifiers::SHIFT, shift);
            mods
        };
        let none = Modifiers::default();
        let (left, right) = (PointerButton::Left, PointerButton::Right);
        let left_by = |state| Response {
            left: state,
            ..Response::idle(WidgetId::from_hash("probe"))
        };
        let right_by = |state| Response {
            right: state,
            ..Response::idle(WidgetId::from_hash("probe"))
        };
        let middle_by = |state| Response {
            middle: state,
            ..Response::idle(WidgetId::from_hash("probe"))
        };
        let both = Response {
            left: clicked(1),
            middle: dragged,
            ..Response::idle(WidgetId::from_hash("probe"))
        };
        let cases = [
            (
                "idle",
                Response::idle(WidgetId::from_hash("probe")),
                none,
                None,
            ),
            (
                "middle drag",
                middle_by(dragged),
                none,
                Some(CanvasGesture::Pan),
            ),
            ("a drag beats a click", both, none, Some(CanvasGesture::Pan)),
            (
                "right drag",
                right_by(dragged),
                none,
                Some(CanvasGesture::Breaker(right)),
            ),
            (
                "shift right drag",
                right_by(dragged),
                mods(false, true),
                Some(CanvasGesture::Tie(right)),
            ),
            (
                "left drag",
                left_by(dragged),
                none,
                Some(CanvasGesture::Select),
            ),
            (
                "shift left drag",
                left_by(dragged),
                mods(false, true),
                Some(CanvasGesture::Select),
            ),
            (
                "ctrl left drag",
                left_by(dragged),
                mods(true, false),
                Some(CanvasGesture::Breaker(left)),
            ),
            (
                "ctrl shift left drag",
                left_by(dragged),
                mods(true, true),
                Some(CanvasGesture::Tie(left)),
            ),
            (
                "right click",
                right_by(clicked(1)),
                none,
                Some(CanvasGesture::NewNode),
            ),
            (
                "double click",
                left_by(clicked(2)),
                none,
                Some(CanvasGesture::NewNode),
            ),
            (
                "left click",
                left_by(clicked(1)),
                none,
                Some(CanvasGesture::Deselect),
            ),
        ];
        for (what, resp, modifiers, gesture) in cases {
            assert_eq!(CanvasGesture::classify(&resp, modifiers), gesture, "{what}");
        }
    }
}
