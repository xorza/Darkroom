//! The graph-editing machinery: forward-only [`intent`] descriptions
//! folded into self-contained undo steps, the packed [`action_stack`]
//! history, and graph publication.

pub(crate) mod action_stack;
pub(crate) mod intent;
pub(crate) mod publish;
