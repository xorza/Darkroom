//! The in-flight gesture controllers, plus the machinery they share.
//!
//! Each owns one gesture's cross-frame state, consumes only the
//! [`CanvasGesture`](crate::gui::pane::graph::CanvasGesture) variant the
//! canvas classified for it, and emits its result as intents rather than
//! editing the document — so the precedence among them lives in one match
//! upstairs and never has to be kept disjoint by hand down here.
//!
//! [`drag_anchor`] and [`slot`] are the shared halves: the press-frame
//! position snapshot behind every drag, and the one-gesture-lifetime slot the
//! rest are built out of.

pub(crate) mod breaker;
pub(crate) mod connection;
pub(crate) mod drag_anchor;
pub(crate) mod new_node;
pub(crate) mod node_menu;
pub(crate) mod pan_zoom;
pub(crate) mod preview_drag;
pub(crate) mod selection;
pub(crate) mod shortcuts;
pub(crate) mod slot;
pub(crate) mod subscription;
