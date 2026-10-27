//! The in-flight gesture controllers, plus the machinery they share.
//!
//! Each owns one gesture's cross-frame state, consumes only the
//! [`CanvasGesture`](crate::gui::pane::graph_ui::CanvasGesture) variant the
//! canvas classified for it, and emits its result as intents rather than
//! editing the document — so the precedence among them lives in one match
//! upstairs and never has to be kept disjoint by hand down here.
//!
//! [`group_drag`] is the shared half: the press-frame position snapshot behind
//! every node drag. The one-gesture-lifetime slot the rest are built out of is
//! [`GestureSlot`](crate::gui::widgets::gesture_slot::GestureSlot), which the
//! image viewer shares.

pub(crate) mod breaker_ui;
pub(crate) mod canvas_gesture;
pub(crate) mod group_drag;
pub(crate) mod new_node_ui;
pub(crate) mod node_menu_ui;
pub(crate) mod pan_zoom;
pub(crate) mod preview_drag;
pub(crate) mod selection_ui;
pub(crate) mod shortcuts;
pub(crate) mod subscription_ui;
pub(crate) mod tie_ui;
pub(crate) mod wire_ui;
