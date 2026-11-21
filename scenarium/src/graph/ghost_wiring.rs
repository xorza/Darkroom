//! [`GhostWiring`]: the bindings and subscriptions a graph keeps and cannot hold.
//!
//! # Decisions
//!
//! - **Ghost wiring is the graph's, in tables of its own beside the live ones.** No reader of
//!   the [`Wiring`](crate::Wiring) sees a ghost, as no reader of the nodes sees a ghost node. As Blender keeps an
//!   undefined node's links in the tree
//!   ([undefined node](https://projects.blender.org/blender/blender/pulls/114783)) and Node-RED's
//!   unknown node "preserves the original configuration and connections"
//!   ([Unknown node](https://flowfuse.com/node-red/core-nodes/unknown/)). Refused: one table
//!   with a live-or-ghost mark on each binding, where one reader that forgets the mark compiles
//!   a ghost.
//! - **An input holds one binding, live or ghost**, and a subscription is made or kept, never
//!   both: the tables share no key ([`InputState`], [`SubscriptionState`]). An edit that binds
//!   an input replaces its ghost, as a wire dropped on a wired input replaces the wire. Refused:
//!   both on one input, where the load that could restore the ghost finds the input taken.
//! - **A ghost port's name and type ride on its ghost.** A ghost holds them for an end no
//!   declaration the graph can read gives: a port its node declares reads the declaration, and a
//!   ghost node's port its saved one. An edit that brings a port back leaves its saved name on
//!   the ghost, which the declaration outranks and the next load clears; no edit rewrites the
//!   ghosts it does not touch. They go when the ghost goes, and an undo brings both back as one
//!   value.
//!   Refused: a table of lost ports for each node, which each removal has to prune and each undo
//!   refill.

use std::collections::BTreeMap;
use std::mem;

use ::serde::{Deserialize, Serialize};

use crate::data::const_value::ConstValue;
use crate::decl::saved_decl::{SavedEvent, SavedPort};
use crate::graph::binding::Binding;
use crate::graph::subscription::Subscription;
use crate::graph::wiring::{self, subscription_touches};
use crate::identity::{EventPort, InputPort, NodeId, OutputPort, PortId};

/// A binding a graph keeps and cannot hold: what was on an input, and the ports as they were
/// where a node no longer declares them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhostBinding {
    /// The input as it was, when no declaration the graph can read gives it: its node no
    /// longer declares it, or is a ghost node that does not know it.
    pub input: Option<SavedPort>,
    pub source: GhostSource,
}

/// What a ghost binding bound its input to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GhostSource {
    Const(ConstValue),
    Wire {
        output: OutputPort,
        /// The output as it was, when no declaration the graph can read gives it.
        saved: Option<SavedPort>,
    },
}

impl GhostBinding {
    /// The binding this was, and is again once the graph can hold it.
    pub fn binding(&self) -> Binding {
        match &self.source {
            GhostSource::Const(value) => Binding::Const(value.clone()),
            GhostSource::Wire { output, .. } => Binding::Wire(*output),
        }
    }

    /// The output a ghost wire reads, or `None` for a constant.
    pub const fn output(&self) -> Option<OutputPort> {
        match &self.source {
            GhostSource::Const(_) => None,
            GhostSource::Wire { output, .. } => Some(*output),
        }
    }

    /// Whether this ghost, on `port`, touches a node `touches` holds, from either end.
    fn touches(&self, port: InputPort, touches: impl Fn(NodeId) -> bool) -> bool {
        touches(port.node_id) || self.output().is_some_and(|output| touches(output.node_id))
    }
}

impl From<Binding> for GhostBinding {
    /// `binding` as a ghost with both of its ports declared where they are.
    fn from(binding: Binding) -> Self {
        Self {
            input: None,
            source: match binding {
                Binding::Const(value) => GhostSource::Const(value),
                Binding::Wire(output) => GhostSource::Wire {
                    output,
                    saved: None,
                },
            },
        }
    }
}

/// A subscription a graph keeps and cannot hold.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhostSubscription {
    /// The event as it was, when no declaration the graph can read gives it.
    pub event: Option<SavedEvent>,
    /// The trigger as it was, likewise: an `Outputs` node's, named as the interface event it
    /// passed out.
    pub trigger: Option<SavedEvent>,
}

/// What one input holds: the value an edit of an input reads and writes, so no edit leaves a
/// live binding and a ghost on one input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputState {
    Unbound,
    Live(Binding),
    Ghost(GhostBinding),
}

impl From<Option<Binding>> for InputState {
    fn from(binding: Option<Binding>) -> Self {
        binding.map_or(Self::Unbound, Self::Live)
    }
}

/// Whether a subscription is made, kept as a ghost, or neither; [`InputState`] for an event
/// edge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubscriptionState {
    Absent,
    Live,
    Ghost(GhostSubscription),
}

/// The bindings and subscriptions a graph keeps and cannot hold, in the shape of its live
/// tables: a binding under the input it was on, a subscription under itself. Also what a
/// removal takes out with a node: each ghost that touches it.
///
/// Read through the methods here and written through [`Graph`](crate::Graph)'s, which keep it
/// apart from the live tables.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhostWiring {
    #[serde(
        default,
        serialize_with = "crate::graph::serde::serialize_pairs",
        deserialize_with = "crate::graph::serde::deserialize_ghost_bindings"
    )]
    pub(super) bindings: BTreeMap<InputPort, GhostBinding>,
    #[serde(
        default,
        serialize_with = "crate::graph::serde::serialize_pairs",
        deserialize_with = "crate::graph::serde::deserialize_ghost_subscriptions"
    )]
    pub(super) subscriptions: BTreeMap<Subscription, GhostSubscription>,
}

impl GhostWiring {
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty() && self.subscriptions.is_empty()
    }

    /// The ghost on `port`, if one is kept there.
    pub fn binding(&self, port: InputPort) -> Option<&GhostBinding> {
        self.bindings.get(&port)
    }

    /// The ghost bindings, in port order.
    pub fn bindings(&self) -> impl ExactSizeIterator<Item = (InputPort, &GhostBinding)> {
        self.bindings.iter().map(|(port, ghost)| (*port, ghost))
    }

    /// The ghosts on `node_id`'s own inputs, in port-id order: one contiguous range, as an
    /// input orders by its node first.
    pub fn bindings_on(&self, node_id: NodeId) -> impl Iterator<Item = (InputPort, &GhostBinding)> {
        self.bindings
            .range(InputPort::new(node_id, PortId::MIN)..)
            .take_while(move |(port, _)| port.node_id == node_id)
            .map(|(port, ghost)| (*port, ghost))
    }

    /// The ghost subscriptions to `emitter`'s events, by event, then subscriber: one contiguous
    /// range, as a subscription orders by its event's node first.
    pub fn subscriptions_from(
        &self,
        emitter: NodeId,
    ) -> impl Iterator<Item = (Subscription, &GhostSubscription)> {
        self.subscriptions
            .range(Subscription::new(EventPort::new(emitter, PortId::MIN), NodeId::nil())..)
            .take_while(move |(subscription, _)| subscription.event.node_id == emitter)
            .map(|(subscription, ghost)| (*subscription, ghost))
    }

    /// How many ghosts touch `node_id`, from either end.
    pub fn count_touching(&self, node_id: NodeId) -> usize {
        let touches = |id: NodeId| id == node_id;
        self.bindings
            .iter()
            .filter(|(port, ghost)| ghost.touches(**port, touches))
            .count()
            + self
                .subscriptions
                .keys()
                .filter(|subscription| subscription_touches(**subscription, touches))
                .count()
    }

    /// The ghost of `subscription`, if one is kept.
    pub fn subscription(&self, subscription: Subscription) -> Option<&GhostSubscription> {
        self.subscriptions.get(&subscription)
    }

    /// The ghost subscriptions, in the order the live ones keep.
    pub fn subscriptions(
        &self,
    ) -> impl ExactSizeIterator<Item = (Subscription, &GhostSubscription)> {
        self.subscriptions
            .iter()
            .map(|(subscription, ghost)| (*subscription, ghost))
    }

    /// Every ghost wire as (consumer input ← producer output); a ghost constant is none.
    pub fn wires(&self) -> impl Iterator<Item = (InputPort, OutputPort)> + '_ {
        self.bindings
            .iter()
            .filter_map(|(port, ghost)| Some((*port, ghost.output()?)))
    }

    /// The ghost wires that read `producer`'s outputs, by input. A scan: the ghost tables are
    /// empty on a healthy document. Refused: a cache of a node's ghost ends per revision.
    pub fn wires_from(&self, producer: NodeId) -> impl Iterator<Item = (InputPort, &GhostBinding)> {
        self.bindings().filter(move |(_, ghost)| {
            ghost
                .output()
                .is_some_and(|output| output.node_id == producer)
        })
    }

    /// The ghost subscriptions that land on `subscriber`'s triggers, by event.
    pub fn subscriptions_to(
        &self,
        subscriber: NodeId,
    ) -> impl Iterator<Item = (Subscription, &GhostSubscription)> {
        self.subscriptions()
            .filter(move |(subscription, _)| subscription.trigger.node_id == subscriber)
    }

    /// Each ghost that touches any of `nodes`, from either end. `nodes` is ascending, each once:
    /// one node is `&[node_id]`.
    #[must_use]
    pub fn touching(&self, nodes: &[NodeId]) -> Self {
        let touches = wiring::touches(nodes);
        Self {
            bindings: self
                .bindings
                .iter()
                .filter(|(port, ghost)| ghost.touches(**port, touches))
                .map(|(port, ghost)| (*port, ghost.clone()))
                .collect(),
            subscriptions: self
                .subscriptions
                .iter()
                .filter(|(subscription, _)| subscription_touches(**subscription, touches))
                .map(|(subscription, ghost)| (*subscription, ghost.clone()))
                .collect(),
        }
    }

    /// Take out each ghost that touches any of `nodes`, as [`Self::touching`] reads it.
    pub(crate) fn remove_touching(&mut self, nodes: &[NodeId]) -> Self {
        let touches = wiring::touches(nodes);
        let (bindings, kept) = mem::take(&mut self.bindings)
            .into_iter()
            .partition(|(port, ghost)| ghost.touches(*port, touches));
        self.bindings = kept;
        let (subscriptions, kept) = mem::take(&mut self.subscriptions)
            .into_iter()
            .partition(|(subscription, _)| subscription_touches(*subscription, touches));
        self.subscriptions = kept;
        Self {
            bindings,
            subscriptions,
        }
    }

    /// Add `other`'s, whose keys this holds none of.
    pub(super) fn extend(&mut self, other: Self) {
        self.bindings.extend(other.bindings);
        self.subscriptions.extend(other.subscriptions);
    }

    /// Whether each ghost touches `node_id`: what a removal's record holds.
    pub(super) fn all_touch(&self, node_id: NodeId) -> bool {
        let touches = |id: NodeId| id == node_id;
        self.bindings
            .iter()
            .all(|(port, ghost)| ghost.touches(*port, touches))
            && self
                .subscriptions
                .keys()
                .all(|subscription| subscription_touches(*subscription, touches))
    }
}
