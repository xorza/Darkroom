//! Per-frame value-request registry: which nodes' runtime values to ask
//! the worker for this run epoch.
//!
//! Kept separate from [`RunState`](crate::gui::run_state::RunState), whose
//! job is projecting the *last finished* run's status/logs/values onto
//! nodes — a backward-looking read of what already happened. This is the
//! forward half of the same protocol: every value-showing surface
//! (inspector panels, image-viewer tabs) calls [`ValueRequests::watch`] as
//! it records itself, declaring its node's value need for the frame; `App`
//! drains the accumulated batch once via [`ValueRequests::take_requests`]
//! into worker sends, tagged with the run epoch [`Self::set_epoch`] last
//! synced from `RunState`.

use std::cell::RefCell;
use std::collections::HashSet;

use scenarium::graph::NodeId;

use crate::core::worker::{RunId, ValueRequest};

/// Per-frame value-watch registration, deduped into one worker request per
/// node per run epoch. `Editor` syncs `run_id` in lockstep with
/// `RunState`'s own (`begin_run` / a cleared run) via [`Self::set_epoch`].
#[derive(Default, Debug)]
pub(crate) struct ValueRequests {
    /// The run epoch pending requests are tagged with — kept in sync with
    /// `RunState::run_id` via [`Self::set_epoch`], not advanced independently.
    run_id: RunId,
    /// Nodes already asked about the current epoch (insert-only; cleared
    /// only by [`Self::set_epoch`]). Dedups so the frame loop sends one
    /// request per node per run — a reply, including a `None` one, doesn't
    /// reopen the node for re-request.
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
    /// the current epoch: each watched node not yet asked about this epoch,
    /// marked asked here and returned for `App` to forward to the worker.
    pub(crate) fn take_requests(&mut self) -> Vec<ValueRequest> {
        let run_id = self.run_id;
        std::mem::take(self.watched.get_mut())
            .into_iter()
            .filter(|&node_id| self.requested.insert(node_id))
            .map(|node_id| ValueRequest { node_id, run_id })
            .collect()
    }

    /// Sync to `run_id`, dropping outdated dedup/pending bookkeeping from
    /// the previous epoch — call alongside `RunState::begin_run` /
    /// `RunState::clear` so requests tag under the epoch those track.
    pub(crate) fn set_epoch(&mut self, run_id: RunId) {
        self.run_id = run_id;
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
    /// `None` reply must not reopen it). A new epoch re-asks under its own
    /// id; watching is per-frame, so each pass re-registers.
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
        assert!(vr.take_requests().is_empty());

        // First pass: both asked; a same-frame duplicate watch is deduped.
        watch_both(&vr);
        vr.watch(a);
        assert_eq!(vr.take_requests(), vec![req(a, 0), req(b, 0)]);

        // Re-watched but already asked this epoch → nothing.
        watch_both(&vr);
        assert!(vr.take_requests().is_empty());

        // A new epoch → both re-asked, tagged under the new id.
        vr.set_epoch(1);
        watch_both(&vr);
        assert_eq!(vr.take_requests(), vec![req(a, 1), req(b, 1)]);
    }
}
