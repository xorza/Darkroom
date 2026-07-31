//! What a pass over the graph pane *reads*: last frame's port and node
//! geometry, this frame's swept interactions, and the region the viewport
//! keeps. Filled before the controllers run and shared by every one of them
//! through [`CanvasCtx`](crate::gui::pane::graph::ctx::CanvasCtx) — so nothing
//! below re-derives what the pass already resolved once.

pub(crate) mod cull;
pub(crate) mod geometry;
pub(crate) mod hits;
pub(crate) mod prepass;
