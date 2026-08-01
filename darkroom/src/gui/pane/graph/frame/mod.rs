//! What a pass over the graph pane *reads*: last frame's port and node
//! geometry, and the region the viewport keeps. Both are resolved once, ahead
//! of the controllers, and shared by every one of them through
//! [`CanvasCtx`](crate::gui::pane::graph::ctx::CanvasCtx) — so nothing below
//! re-derives what the pass already settled.

pub(crate) mod cull;
pub(crate) mod geometry;
