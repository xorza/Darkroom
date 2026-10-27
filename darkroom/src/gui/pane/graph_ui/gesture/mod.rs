//! The in-flight gesture controllers, plus the machinery they share.
//!
//! Each owns one gesture's cross-frame state, consumes only the
//! [`CanvasGesture`](crate::gui::pane::graph_ui::prepass_ctx::CanvasGesture) variant the
//! canvas classified for it, and emits its result as intents rather than
//! editing the document — so the precedence among them lives in one match
//! upstairs and never has to be kept disjoint by hand down here.
//!
//! The shared halves: [`group_drag`], the press-frame position snapshot behind
//! every node drag; [`glyph_drag`], a drag from one glyph to another; and
//! [`item_menu`] over [`anchored_menu`], the popups; and [`button_stroke`], a stroke held by
//! the button that latched it. A gesture in flight is an `Option` of its state.

pub(crate) mod anchored_menu;
pub(crate) mod breaker_ui;
pub(crate) mod button_stroke;
pub(crate) mod glyph_drag;
pub(crate) mod group_drag;
pub(crate) mod item_menu;
pub(crate) mod new_node_ui;
pub(crate) mod node_menu_ui;
pub(crate) mod pan_zoom;
pub(crate) mod preview_drag;
pub(crate) mod segments;
pub(crate) mod selection_ui;
pub(crate) mod shortcuts;
pub(crate) mod subscription_ui;
pub(crate) mod tie_ui;
pub(crate) mod wire_ui;
