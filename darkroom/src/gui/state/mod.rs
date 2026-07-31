//! The runtime projections `App` owns and the frame reads: the last run's
//! per-node verdicts, the preview values and textures those runs published,
//! and this process's own footprint.
//!
//! None of it draws. It is lent to the UI tree through
//! [`AppCtx`](crate::gui::app::ctx::AppCtx), and `App` is its only writer —
//! which is why it lives beside the shell rather than among the widgets that
//! read it.

pub(crate) mod preview_store;
pub(crate) mod process_memory;
pub(crate) mod run_state;
