//! The records a reversible removal leaves behind: everything needed to put
//! back exactly what was taken out.
//!
//! A [`DetachedNode`] carries a removed node with all the wiring that touched
//! it; [`DetachedGraphInput`] and [`DetachedGraphOutput`] carry a removed
//! interface port with the interior and instance edges it severed. Each is
//! produced by a `Graph` snapshot or detach and consumed by the matching
//! attach, so undo restores wiring rather than reconstructing it — which is
//! why they own their contents instead of borrowing.
//!
//! Each also knows what a well-formed record looks like: the `assert_*` methods
//! here are the preflight a mutation runs before it commits, so a malformed
//! record fails at the call that supplied it rather than halfway through the
//! graph edit.

use ::serde::{Deserialize, Serialize};

use crate::graph::Binding;
use crate::graph::identity::NodeId;
use crate::graph::identity::{InputPort, OutputPort, Subscription};
use crate::graph::node::Node;
use crate::graph::node::definition::{FuncInput, FuncOutput};
use crate::graph::{BindingEntry, binding_touches, subscription_touches};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DetachedNode {
    pub node_id: NodeId,
    pub node: Node,
    pub bindings: Vec<BindingEntry>,
    pub subscriptions: Vec<Subscription>,
}

impl DetachedNode {
    /// Panic unless this record is well formed: a real node id, and wiring that
    /// touches it, listed in the ascending order the graph's own side tables
    /// keep. A malformed record is a caller logic error, never user input.
    pub(super) fn assert_valid(&self) {
        assert!(!self.node_id.is_nil(), "detached node id must not be nil");
        assert!(
            self.bindings.iter().all(|entry| binding_touches(
                entry.port,
                &entry.binding,
                self.node_id
            )),
            "detached bindings must touch the detached node"
        );
        assert!(
            self.bindings
                .windows(2)
                .all(|entries| entries[0].port < entries[1].port),
            "detached bindings must have unique ordered ports"
        );
        assert!(
            self.subscriptions
                .iter()
                .all(|subscription| subscription_touches(subscription, self.node_id)),
            "detached subscriptions must touch the detached node"
        );
        assert!(
            self.subscriptions
                .windows(2)
                .all(|subscriptions| subscriptions[0] < subscriptions[1]),
            "detached subscriptions must be unique and ordered"
        );
    }
}

/// A subgraph *input* removed from the interface at `idx`: its spec, the
/// interior edges the boundary output fed, and the owning graph's instance
/// bindings into the slot. Ports above `idx` shift down on detach and back up
/// on attach.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DetachedGraphInput {
    pub idx: usize,
    pub(super) spec: FuncInput,
    /// Interior consumers fed by the `GraphInput` boundary output `idx`.
    pub(super) interior: Vec<BindingEntry>,
    /// Owning-graph bindings on instance input `idx`.
    pub(super) parent: Vec<BindingEntry>,
}

/// A subgraph *output* removed from the interface at `idx` — the output-side
/// mirror of [`DetachedGraphInput`]: interior wiring is the binding *on* the
/// `GraphOutput` boundary input `idx`, parent wiring is every consumer of
/// instance output `idx`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DetachedGraphOutput {
    pub idx: usize,
    pub(super) spec: FuncOutput,
    /// The binding on the `GraphOutput` boundary input `idx`.
    pub(super) interior: Vec<BindingEntry>,
    /// Owning-graph consumers bound to instance output `idx`.
    pub(super) parent: Vec<BindingEntry>,
}

impl DetachedGraphInput {
    /// Panic unless every recorded edge sits on this record's slot: instance
    /// bindings on instance input `idx`, interior edges fed by the boundary
    /// output `idx`. A `None` boundary — no inbound boundary node
    /// in the child body — admits only a record with no interior wiring. Runs
    /// before `attach` mutates anything, so a malformed record can't
    /// half-apply.
    pub(super) fn assert_targets_slot(&self, instances: &[NodeId], boundary: Option<NodeId>) {
        for entry in &self.parent {
            assert!(
                entry.port.port_idx == self.idx && instances.contains(&entry.port.node_id),
                "detached instance binding does not sit on the detached input slot"
            );
        }
        let Some(boundary) = boundary else {
            assert!(
                self.interior.is_empty(),
                "detached interior wiring without a boundary node"
            );
            return;
        };
        let slot = OutputPort::new(boundary, self.idx);
        for entry in &self.interior {
            assert!(
                matches!(&entry.binding, Binding::Bind(src) if *src == slot),
                "detached interior edge is not fed by the detached input slot"
            );
        }
    }
}

impl DetachedGraphOutput {
    /// The output-side mirror of
    /// [`DetachedGraphInput::assert_targets_slot`]: consumers must *read*
    /// instance output `idx`, while the lone interior binding sits *on* the
    /// boundary input `idx`.
    pub(super) fn assert_targets_slot(&self, instances: &[NodeId], boundary: Option<NodeId>) {
        for entry in &self.parent {
            assert!(
                matches!(&entry.binding, Binding::Bind(src)
                    if src.port_idx == self.idx && instances.contains(&src.node_id)),
                "detached consumer binding does not read the detached output slot"
            );
        }
        let Some(boundary) = boundary else {
            assert!(
                self.interior.is_empty(),
                "detached interior wiring without a boundary node"
            );
            return;
        };
        for entry in &self.interior {
            assert!(
                entry.port == InputPort::new(boundary, self.idx),
                "detached interior binding does not sit on the detached output slot"
            );
        }
    }
}
