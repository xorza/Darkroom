//! [`NodeRuntime`]: one node's cross-run lambda state.

use crate::graph::identity::FuncRef;
use crate::runtime::any_state::AnyState;
use crate::runtime::shared_any_state::SharedAnyState;

/// One node's persistent `state` and `event_state`, owned by the implementation
/// recorded in `owner`.
///
/// Apart from the node's cached output: the output is digest-keyed, while this
/// survives installs untouched until [`reown`](Self::reown) sees a different
/// implementation.
#[derive(Debug)]
pub(crate) struct NodeRuntime {
    pub(crate) owner: FuncRef,
    pub(crate) state: AnyState,
    pub(crate) event_state: SharedAnyState,
}

impl NodeRuntime {
    /// The state a node gains when an install first brings it in: empty, owned
    /// by the implementation compiled for it.
    pub(super) fn new(owner: FuncRef) -> Self {
        Self {
            owner,
            state: AnyState::default(),
            event_state: SharedAnyState::default(),
        }
    }

    /// Re-stamp the state for the installed node's implementation, dropping
    /// `state` and `event_state` when a different one owned them — a changed
    /// function must not inherit state written by its predecessor.
    pub(super) fn reown(&mut self, owner: FuncRef) {
        if self.owner == owner {
            return;
        }
        self.owner = owner;
        self.state.clear();
        self.event_state = SharedAnyState::default();
    }
}
