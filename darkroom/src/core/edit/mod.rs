//! The graph-editing machinery: forward-only [`intent`] descriptions
//! folded into self-contained undo steps, the packed [`action_stack`]
//! history, and the derived-state [`reconcile`] pass that keeps each
//! graph's interface in sync with its interior wiring.

pub(crate) mod action_stack;
pub(crate) mod intent;
pub(crate) mod publish;
pub(crate) mod reconcile;
