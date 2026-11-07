//! What every UI surface speaks: the root of the frame's read-only context chain
//! ([`app_ctx`]), the bundle each level hands down ([`stage`]), what a record takes past it ([`window_parts`], [`requests`]), and the
//! commands a surface raises for the shell ([`app_command`], [`framing`]). Data only — the levels
//! that act on it import it, and it imports none of them.
//!
//! A level takes `(ui, stage)`, on the input stage and on the record path alike. It adds to its
//! stage's context and hands it down, and nothing returns upward.

pub(crate) mod app_command;
pub(crate) mod app_ctx;
pub(crate) mod framing;
pub(crate) mod requests;
pub(crate) mod shortcuts;
pub(crate) mod stage;
pub(crate) mod window_parts;
