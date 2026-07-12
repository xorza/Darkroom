//! Per-frame value-request registry: which nodes' runtime values to ask
//! the worker for this run epoch.
//!
//! Kept separate from [`RunState`](crate::gui::run_state::RunState), whose
//! job is projecting the *last finished* run's status/logs/values onto
//! nodes — a backward-looking read of what already happened. This is the
//! forward half of the same protocol: every value-showing surface
//! (inspector panels, image-viewer tabs, Preview nodes) calls
//! [`ValueRequests::watch`] as it records itself, declaring its node's
//! value need for the frame; `App` drains the accumulated batch once via
//! [`ValueRequests::take_requests`] into worker sends, tagged with
//! `RunState`'s current run epoch.

use std::cell::RefCell;
use std::collections::HashSet;

use scenarium::graph::NodeId;

use crate::core::worker::{RunId, ValueRequest};

/// Per-frame value-watch registration, deduped into one worker request per
/// node per run epoch. `Editor` resets it in lockstep with `RunState`'s
/// epoch (`begin_run` / a cleared run) via [`Self::reset`].
#[derive(Default, Debug)]
pub(crate) struct ValueRequests {
    /// Nodes already asked about the current epoch (insert-only; cleared
    /// only by [`Self::reset`]). Dedups so the frame loop sends one request
    /// per node per run — a reply, including a `None` one, doesn't reopen
    /// the node for re-request.
    requested: HashSet<NodeId>,
    /// Per-frame registry: every surface showing runtime values calls
    /// [`Self::watch`] **as it is recorded**, declaring its node's value
    /// need; `App` drains the batch once through [`Self::take_requests`] at
    /// frame end. `RefCell` so a widget can register through a shared
    /// `&ValueRequests` the record threads (no `&mut` reaches the record).
    /// Re-registration each frame beats a retained subscription — nothing
    /// needs an unregister path when a panel closes, a tab dies with its
    /// node, or a node scrolls off-screen.
    watched: RefCell<Vec<NodeId>>,
}

impl ValueRequests {
    /// Register interest in `node_id`'s runtime values for this frame.
    /// Called by every value-showing widget as it records itself.
    pub(crate) fn watch(&self, node_id: NodeId) {
        self.watched.borrow_mut().push(node_id);
    }

    /// Drain the frame's registry into pending value requests tagged with
    /// `run_id`: each watched node not yet asked about this epoch, marked
    /// asked here and returned for `App` to forward to the worker.
    pub(crate) fn take_requests(&mut self, run_id: RunId) -> Vec<ValueRequest> {
        std::mem::take(self.watched.get_mut())
            .into_iter()
            .filter(|&node_id| self.requested.insert(node_id))
            .map(|node_id| ValueRequest { node_id, run_id })
            .collect()
    }

    /// Reset dedup + pending registration for a new run epoch (or a
    /// dropped run) — call alongside `RunState::begin_run` / `RunState::clear`.
    pub(crate) fn reset(&mut self) {
        self.requested.clear();
        self.watched.get_mut().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nid(n: u128) -> NodeId {
        NodeId::from_u128(n)
    }

    /// Asks for each watched node once per epoch, then nothing more — a
    /// node already asked is not re-requested even if no value landed (a
    /// `None` reply must not reopen it). A reset (new epoch) re-asks;
    /// watching is per-frame, so each pass re-registers.
    #[test]
    fn take_requests_asks_each_watched_node_once_per_epoch() {
        let (a, b) = (nid(1), nid(2));
        let req = |node_id, run_id| ValueRequest { node_id, run_id };
        let watch_both = |vr: &ValueRequests| {
            vr.watch(a);
            vr.watch(b);
        };
        let mut vr = ValueRequests::default();

        // Nothing watched → nothing requested.
        assert!(vr.take_requests(0).is_empty());

        // First pass: both asked; a same-frame duplicate watch is deduped.
        watch_both(&vr);
        vr.watch(a);
        assert_eq!(vr.take_requests(0), vec![req(a, 0), req(b, 0)]);

        // Re-watched but already asked this epoch → nothing.
        watch_both(&vr);
        assert!(vr.take_requests(0).is_empty());

        // A reset (new epoch) → both re-asked under the new epoch's id.
        vr.reset();
        watch_both(&vr);
        assert_eq!(vr.take_requests(1), vec![req(a, 1), req(b, 1)]);
    }
}
