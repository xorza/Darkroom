//! The record pass's drawing helpers: the wires between node bodies, the
//! inspection panels over them, and the anchoring a popup opens against.
//! Everything here takes a [`DrawCtx`](crate::gui::pane::graph::ctx::DrawCtx)
//! and paints; none of it owns gesture state.

pub(crate) mod anchored_menu;
pub(crate) mod inspector;
pub(crate) mod wire;
