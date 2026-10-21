//! What every UI surface speaks: the frame's read-only context chain
//! ([`app_ctx`], [`window_ctx`]), what a frame may write ([`frame_writes`],
//! [`requests`]), and the commands a surface raises for the shell
//! ([`app_command`]). Data only — the levels that act on it import it, and it
//! imports none of them.

pub(crate) mod app_command;
pub(crate) mod app_ctx;
pub(crate) mod frame_writes;
pub(crate) mod requests;
pub(crate) mod window_ctx;
