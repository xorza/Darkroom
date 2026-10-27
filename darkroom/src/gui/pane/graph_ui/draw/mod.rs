//! The record pass's drawing helpers: the wires between node bodies, the ties
//! that gather them, the inspection panels over them, and the anchoring a
//! popup opens against.
//! Everything here takes a [`DrawCtx`](crate::gui::pane::graph_ui::draw_ctx::DrawCtx)
//! and draws; none of it owns gesture state.

pub(crate) mod anchored_menu;
pub(crate) mod context_menu;
pub(crate) mod inspectors;
pub(crate) mod tie;
pub(crate) mod wire;
