//! What every UI surface speaks: the root of the frame's read-only context chain
//! ([`app_ctx`]), the bundles the update and record paths hand down ([`input_stage`],
//! [`record_stage`]), what a record takes past them ([`window_parts`], [`requests`]), and the
//! commands a surface raises for the shell ([`app_command`], [`framing`]). Data only — the levels
//! that act on it import it, and it imports none of them.

pub(crate) mod app_command;
pub(crate) mod app_ctx;
pub(crate) mod framing;
pub(crate) mod input_stage;
pub(crate) mod record_stage;
pub(crate) mod requests;
pub(crate) mod shortcuts;
pub(crate) mod window_parts;
