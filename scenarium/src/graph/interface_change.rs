//! [`InterfaceChange`]: the reversible record of an edit of a compound's interface.
//!
//! # Decisions
//!
//! - **An interface edit is the set's, and it changes another graph only by the wiring it
//!   refuses.** A reorder and a rename change no binding anywhere, as wiring names a port by id.
//!   A removed port refuses the wires and constants on it in every graph, a retyped one those it
//!   no longer admits, and undo puts them back: the record is the two interfaces and those
//!   bindings. Refused: the interface as a field a host sets, which leaves the refused wires in
//!   other graphs to each host.
//! - **What it refuses is dropped or kept as ghosts, as the caller says** ([`RefusedFate`]). A
//!   port a person removes or retypes takes its wiring, as they pointed at it; an update from a
//!   library changes the interface under them, as a func update does, so it keeps the wiring as
//!   ghosts, each lost port's name and type from the interface it replaces. Refused: ghosts for
//!   both, as Unreal leaves a removed parameter on each call site until you "refresh node or
//!   break links to remove pin"
//!   ([thread](https://forums.unrealengine.com/t/new-dead-pin-functionality-buggy/404854)),
//!   which leaves a row on every instance for a port removed on purpose; a drop for both, which
//!   takes your wires for another author's edit.
//! - **The plan reads a copy of the set under the new interface**, so the one admission rule
//!   decides what goes. Cost: one copy of the set's graphs per interface edit, a person's action.
//!   Refused: a second rule that reads the types beside [`Graph::admit`](crate::Graph::admit),
//!   which would drift from it.

use serde::{Deserialize, Serialize};

use crate::graph::RefusedWiring;
use crate::graph::binding::Binding;
use crate::graph::ghost_wiring::{
    GhostBinding, GhostSource, GhostSubscription, InputState, SubscriptionState,
};
use crate::graph::graph_set::GraphSet;
use crate::graph::identity::{CompoundId, GraphId, InputPort, OutputPort};
use crate::graph::interface::Interface;
use crate::graph::node_decl::NodeTriggers;
use crate::graph::saved_decl::{SavedEvent, SavedPort};
use crate::graph::subscription::Subscription;
use crate::library::Library;

/// An edit of one compound's interface: the interface before and after, each binding the new
/// interface does not hold, in any graph, and what becomes of them. [`GraphSet::write_interface`]
/// lands either side.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterfaceChange {
    id: CompoundId,
    before: Interface,
    after: Interface,
    refused: Vec<RefusedBinding>,
    refused_subscriptions: Vec<RefusedSubscription>,
    fate: RefusedFate,
}

/// A subscription an interface edit refuses, to or from an event or a trigger the new interface
/// lacks, as the ghost it would be, with the name of an event it no longer gives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusedSubscription {
    pub graph: GraphId,
    pub subscription: Subscription,
    pub ghost: GhostSubscription,
}

/// What an interface edit does with each binding the new interface refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefusedFate {
    /// Takes it out, as a person's edit of a port does.
    Drop,
    /// Keeps it as a ghost, as an update from a library does.
    Ghost,
}

/// A binding an interface edit refuses: on an instance, from an instance's output, or on or from
/// a boundary node inside, as the ghost it would be, with the name and type of each of its ports
/// the new interface no longer gives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusedBinding {
    pub graph: GraphId,
    pub port: InputPort,
    pub ghost: GhostBinding,
}

impl RefusedBinding {
    /// The binding the graph held.
    pub fn binding(&self) -> Binding {
        self.ghost.binding()
    }
}

/// Which side of an [`InterfaceChange`] a write lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeSide {
    Before,
    After,
}

impl InterfaceChange {
    pub const fn id(&self) -> CompoundId {
        self.id
    }

    pub const fn before(&self) -> &Interface {
        &self.before
    }

    pub const fn after(&self) -> &Interface {
        &self.after
    }

    /// In graph order, then in the order each graph keeps its bindings.
    pub fn refused(&self) -> &[RefusedBinding] {
        &self.refused
    }

    /// In graph order, then in the order each graph keeps its subscriptions.
    pub fn refused_subscriptions(&self) -> &[RefusedSubscription] {
        &self.refused_subscriptions
    }

    pub const fn fate(&self) -> RefusedFate {
        self.fate
    }
}

impl GraphSet {
    /// What setting compound `id`'s interface to `to` changes: each binding, in any graph, that
    /// the new interface does not hold — on or from an instance of `id` whose port it lacks or no
    /// longer admits, and on or from a boundary node of `id`'s graph likewise — which `fate`
    /// drops or keeps as ghosts.
    ///
    /// # Panics
    /// If this set holds no compound under `id`.
    pub fn plan_interface(
        &self,
        id: CompoundId,
        to: Interface,
        library: &Library,
        fate: RefusedFate,
    ) -> InterfaceChange {
        let before = self
            .compound(id)
            .expect("an interface edit's compound is in the set")
            .interface
            .clone();
        let mut after = self.clone();
        after.set_interface(id, to.clone());
        let mut refused = Vec::new();
        let mut refused_subscriptions = Vec::new();
        for (graph, held) in after.graphs() {
            let scope = after.scope(library, graph);
            let was = self.scope(library, graph);
            let was_graph = self.graph(graph);
            // The name and type a port had, where the new interface no longer gives it.
            let lost_input = |port: InputPort| {
                let node = held
                    .find(port.node_id)
                    .expect("a refused binding's node is held");
                if node.decl(scope).ports().input(port.port).is_some() {
                    return None;
                }
                let decl = was_graph.find(port.node_id)?.decl(was);
                let input = decl.ports().input(port.port)?;
                Some(SavedPort {
                    name: input.name.clone(),
                    data_type: input.data_type,
                })
            };
            let lost_output = |port: OutputPort| {
                let node = held
                    .find(port.node_id)
                    .expect("a refused wire's producer is held");
                if node.decl(scope).ports().output(port.port).is_some() {
                    return None;
                }
                let decl = was_graph.find(port.node_id)?.decl(was);
                let output = decl.ports().output(port.port)?;
                Some(SavedPort {
                    name: output.name.clone(),
                    data_type: output.data_type,
                })
            };
            for refusal in held.refused_wiring(scope) {
                let port = match refusal {
                    RefusedWiring::Binding { port, .. } => port,
                    RefusedWiring::Subscription { subscription, .. } => {
                        let emitter = held
                            .find(subscription.event.node_id)
                            .expect("a refused subscription's emitter is held");
                        let event = if emitter
                            .decl(scope)
                            .event(subscription.event.event)
                            .is_some()
                        {
                            None
                        } else {
                            was_graph
                                .find(subscription.event.node_id)
                                .and_then(|node| node.decl(was).event(subscription.event.event))
                                .map(|event| SavedEvent {
                                    name: event.name.to_owned(),
                                })
                        };
                        let trigger = subscription.trigger;
                        let subscriber = held
                            .find(trigger.node_id)
                            .expect("a refused subscription's subscriber is held");
                        let trigger = if subscriber.decl(scope).triggers().contains(trigger.trigger)
                        {
                            None
                        } else {
                            was_graph
                                .find(trigger.node_id)
                                .and_then(|node| match node.decl(was).triggers() {
                                    NodeTriggers::PerEvent(events) => {
                                        events.iter().find(|event| event.id == trigger.trigger)
                                    }
                                    NodeTriggers::None | NodeTriggers::Own => None,
                                })
                                .map(|event| SavedEvent {
                                    name: event.name.clone(),
                                })
                        };
                        refused_subscriptions.push(RefusedSubscription {
                            graph,
                            subscription,
                            ghost: GhostSubscription { event, trigger },
                        });
                        continue;
                    }
                };
                let source = match held.bindings[&port].clone() {
                    Binding::Const(value) => GhostSource::Const(value),
                    Binding::Wire(output) => GhostSource::Wire {
                        output,
                        saved: lost_output(output),
                    },
                };
                refused.push(RefusedBinding {
                    graph,
                    port,
                    ghost: GhostBinding {
                        input: lost_input(port),
                        source,
                    },
                });
            }
        }
        InterfaceChange {
            id,
            before,
            after: to,
            refused,
            refused_subscriptions,
            fate,
        }
    }

    /// Land `side` of `change`: its interface, with each refused binding taken out, or kept as
    /// a ghost, for [`ChangeSide::After`], and put back live for [`ChangeSide::Before`].
    ///
    /// # Panics
    /// If the set is not on the other side of `change`: an input does not hold what was
    /// recorded.
    pub fn write_interface(&mut self, change: &InterfaceChange, side: ChangeSide) {
        // What an input holds on either side of the change.
        let halves = |refused: &RefusedBinding| {
            let live = InputState::Live(refused.binding());
            let after = match change.fate {
                RefusedFate::Drop => InputState::Unbound,
                RefusedFate::Ghost => InputState::Ghost(refused.ghost.clone()),
            };
            match side {
                ChangeSide::After => [live, after],
                ChangeSide::Before => [after, live],
            }
        };
        if side == ChangeSide::Before {
            self.set_interface(change.id, change.before.clone());
        }
        for refused in &change.refused {
            let [from, to] = halves(refused);
            let graph = self.graph_mut(refused.graph);
            assert_eq!(
                graph.input_state(refused.port),
                from,
                "an interface edit lands on the input it recorded"
            );
            graph.set_input_state(refused.port, to);
        }
        for refused in &change.refused_subscriptions {
            let after = match change.fate {
                RefusedFate::Drop => SubscriptionState::Absent,
                RefusedFate::Ghost => SubscriptionState::Ghost(refused.ghost.clone()),
            };
            let [from, to] = match side {
                ChangeSide::After => [SubscriptionState::Live, after],
                ChangeSide::Before => [after, SubscriptionState::Live],
            };
            let graph = self.graph_mut(refused.graph);
            assert_eq!(
                graph.subscription_state(refused.subscription),
                from,
                "an interface edit lands on the subscription it recorded"
            );
            graph.set_subscription_state(refused.subscription, to);
        }
        if side == ChangeSide::After {
            self.set_interface(change.id, change.after.clone());
        }
    }
}
