//! [`Wiring`]: the bindings and subscriptions a graph holds.

use std::collections::{BTreeMap, BTreeSet};
use std::mem;

use ::serde::{Deserialize, Serialize};

use crate::decl::ports::Ports;
use crate::graph::binding::Binding;
use crate::graph::subscription::Subscription;
use crate::identity::{EventPort, InputPort, NodeId, OutputPort, PortId};

/// The bindings and subscriptions a graph holds, and what a removal takes out with its nodes:
/// [`GhostWiring`](crate::GhostWiring)'s live counterpart, with its method names. An input holds
/// one binding by construction, as the table is keyed by it.
///
/// A graph's is read through [`Graph::wiring`](crate::Graph::wiring) and written through the
/// graph's own methods, which keep it apart from the ghost wiring; a value of its own, as an edit
/// assembles one, is written here.
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Wiring {
    /// Data wiring, keyed by consumer input port; absence means unbound. A `BTreeMap` keeps
    /// serialization deterministic and a node's ports contiguous. Serialized as `(port, binding)`
    /// pairs decoded entry by entry, so a document that repeats a port is refused rather than
    /// loaded with only the last binding.
    #[serde(
        serialize_with = "crate::graph::serde::serialize_pairs",
        deserialize_with = "crate::graph::serde::deserialize_bindings"
    )]
    pub(super) bindings: BTreeMap<InputPort, Binding>,
    /// Event wiring: every (emitter event → subscriber) edge, flat. A `BTreeSet` dedups and keeps
    /// serialization and compile order deterministic.
    pub(super) subscriptions: BTreeSet<Subscription>,
}

impl Wiring {
    /// The const bindings a fresh node `node_id` of `ports` starts with: one per input that
    /// declares a default, on that input.
    pub fn defaults(node_id: NodeId, ports: &Ports) -> Self {
        let mut wiring = Self::default();
        for input in &ports.inputs {
            if let Some(default) = &input.default_value {
                wiring.bind(
                    InputPort::new(node_id, input.id),
                    Binding::Const(default.clone()),
                );
            }
        }
        wiring
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty() && self.subscriptions.is_empty()
    }

    /// The binding on `port`, if it is bound.
    pub fn binding(&self, port: InputPort) -> Option<&Binding> {
        self.bindings.get(&port)
    }

    /// The bindings, in port order.
    pub fn bindings(&self) -> impl ExactSizeIterator<Item = (InputPort, &Binding)> {
        self.bindings.iter().map(|(port, binding)| (*port, binding))
    }

    /// The bindings on `node_id`'s own inputs, in port-id order: one contiguous range, as an
    /// input orders by its node first.
    pub fn bindings_on(&self, node_id: NodeId) -> impl Iterator<Item = (InputPort, &Binding)> {
        self.bindings
            .range(InputPort::new(node_id, PortId::MIN)..)
            .take_while(move |(port, _)| port.node_id == node_id)
            .map(|(port, binding)| (*port, binding))
    }

    /// Every wire as (consumer input ← producer output); a constant is none.
    pub fn wires(&self) -> impl Iterator<Item = (InputPort, OutputPort)> + '_ {
        self.bindings
            .iter()
            .filter_map(|(port, binding)| match binding {
                Binding::Wire(output) => Some((*port, *output)),
                Binding::Const(_) => None,
            })
    }

    /// Whether `subscription` is made.
    pub fn is_subscribed(&self, subscription: Subscription) -> bool {
        self.subscriptions.contains(&subscription)
    }

    /// The subscriptions, by event, then trigger.
    pub fn subscriptions(&self) -> impl ExactSizeIterator<Item = Subscription> + '_ {
        self.subscriptions.iter().copied()
    }

    /// The subscriptions to `emitter`'s events, by event, then trigger: one contiguous range, as
    /// a subscription orders by its event's node first.
    pub fn subscriptions_from(&self, emitter: NodeId) -> impl Iterator<Item = Subscription> + '_ {
        self.subscriptions
            .range(Subscription::new(EventPort::new(emitter, PortId::MIN), NodeId::nil())..)
            .take_while(move |subscription| subscription.event.node_id == emitter)
            .copied()
    }

    /// Bind `port` to `binding`, handing back what it was bound to.
    pub fn bind(&mut self, port: InputPort, binding: Binding) -> Option<Binding> {
        self.bindings.insert(port, binding)
    }

    /// Unbind `port`, handing back what it was bound to.
    pub fn unbind(&mut self, port: InputPort) -> Option<Binding> {
        self.bindings.remove(&port)
    }

    /// Make `subscription`; whether it was not made yet.
    pub fn subscribe(&mut self, subscription: Subscription) -> bool {
        self.subscriptions.insert(subscription)
    }

    /// The wiring that touches any of `nodes`, from either end. `nodes` is ascending, each once:
    /// one node is `&[node_id]`.
    #[must_use]
    pub fn touching(&self, nodes: &[NodeId]) -> Self {
        let touches = touches(nodes);
        Self {
            bindings: self
                .bindings
                .iter()
                .filter(|(port, binding)| binding_touches(**port, binding, touches))
                .map(|(port, binding)| (*port, binding.clone()))
                .collect(),
            subscriptions: self
                .subscriptions
                .iter()
                .copied()
                .filter(|subscription| subscription_touches(*subscription, touches))
                .collect(),
        }
    }

    /// Take out the wiring that touches any of `nodes`, as [`Self::touching`] reads it.
    pub(crate) fn remove_touching(&mut self, nodes: &[NodeId]) -> Self {
        let touches = touches(nodes);
        let (bindings, kept) = mem::take(&mut self.bindings)
            .into_iter()
            .partition(|(port, binding)| binding_touches(*port, binding, touches));
        self.bindings = kept;
        let (subscriptions, kept) = mem::take(&mut self.subscriptions)
            .into_iter()
            .partition(|subscription| subscription_touches(*subscription, touches));
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
}

impl FromIterator<(InputPort, Binding)> for Wiring {
    /// The bindings of `pairs`, no subscription; a port given twice keeps the last, as a map
    /// built from pairs does.
    fn from_iter<I: IntoIterator<Item = (InputPort, Binding)>>(pairs: I) -> Self {
        Self {
            bindings: pairs.into_iter().collect(),
            subscriptions: BTreeSet::new(),
        }
    }
}

impl<const N: usize> From<[(InputPort, Binding); N]> for Wiring {
    fn from(pairs: [(InputPort, Binding); N]) -> Self {
        Self::from_iter(pairs)
    }
}

/// Whether a node is one of `nodes`, which is ascending and each once.
pub(super) fn touches(nodes: &[NodeId]) -> impl Fn(NodeId) -> bool + Copy + '_ {
    debug_assert!(nodes.is_sorted_by(|left, right| left < right));
    move |node_id| nodes.binary_search(&node_id).is_ok()
}

fn binding_touches(port: InputPort, binding: &Binding, touches: impl Fn(NodeId) -> bool) -> bool {
    touches(port.node_id) || matches!(binding, Binding::Wire(output) if touches(output.node_id))
}

pub(super) fn subscription_touches(
    subscription: Subscription,
    touches: impl Fn(NodeId) -> bool,
) -> bool {
    touches(subscription.event.node_id) || touches(subscription.trigger.node_id)
}
