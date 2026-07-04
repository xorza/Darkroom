//! The graph-editing machinery: forward-only [`intent`] descriptions
//! folded into self-contained undo steps, the packed [`action_stack`]
//! history, and the derived-state [`reconcile`] pass that keeps each
//! subgraph's interface in sync with its interior wiring.

pub mod action_stack;
pub mod intent;
pub mod publish;
pub mod reconcile;
