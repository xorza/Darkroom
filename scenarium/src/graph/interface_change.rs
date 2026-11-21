//! [`InterfaceChange`]: the reversible record of an edit of a compound's interface.
//!
//! # Decisions
//!
//! - **An interface edit is the set's, and it changes another graph only by the wiring it
//!   refuses and the ghosts it names.** A reorder and a rename change no binding anywhere, as
//!   wiring names a port by id. A removed port refuses the wires and constants on it in every
//!   graph, a retyped one those it no longer admits, and a ghost already kept that names a removed
//!   port takes its name, as a func's remap names one; undo puts each back: the record is the two
//!   interfaces, those bindings and those ghosts. Refused: the interface as a field a host sets,
//!   which leaves the refused wires in other graphs to each host.
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

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::decl::saved_decl::{SavedEvent, SavedPort};
use crate::graph::RefusedWiring;
use crate::graph::binding::Binding;
use crate::graph::ghost_wiring::{
    GhostBinding, GhostSource, GhostSubscription, GhostWiring, InputState, SubscriptionState,
};
use crate::graph::graph_set::GraphSet;
use crate::graph::interface::Interface;
use crate::graph::node_decl::NodeTriggers;
use crate::identity::{CompoundId, EventPort, GraphId, InputPort, OutputPort, TriggerPort};
use crate::library::Library;

/// An edit of one compound's interface: the interface before and after, what it changes of each
/// graph's wiring, and what becomes of the wiring it refuses. [`GraphSet::write_interface`] lands
/// either side.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterfaceChange {
    id: CompoundId,
    before: Interface,
    after: Interface,
    fate: RefusedFate,
    /// Each graph the edit changes the wiring of, and how.
    #[serde(
        serialize_with = "crate::graph::serde::serialize_pairs",
        deserialize_with = "crate::graph::serde::deserialize_wiring_changes"
    )]
    graphs: BTreeMap<GraphId, WiringChange>,
}

/// What an interface edit changes of one graph's wiring, as ghosts under the keys the graph keeps
/// them by.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WiringChange {
    /// Each binding and subscription the new interface refuses, as the ghost it would be: with
    /// the name and type of each of its ends the new interface no longer gives.
    refused: GhostWiring,
    /// Each ghost already kept that the edit names an end of, as it was.
    named_before: GhostWiring,
    /// The same ghosts, named: each end the edit removes under the name the old interface gave it.
    named_after: GhostWiring,
}

impl WiringChange {
    /// Each binding and subscription the new interface refuses, as the ghost it would be.
    pub const fn refused(&self) -> &GhostWiring {
        &self.refused
    }

    fn is_empty(&self) -> bool {
        self.refused.is_empty() && self.named_before.is_empty()
    }
}

/// What an interface edit does with each binding the new interface refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefusedFate {
    /// Takes it out, as a person's edit of a port does.
    Drop,
    /// Keeps it as a ghost, as an update from a library does.
    Ghost,
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

    pub const fn fate(&self) -> RefusedFate {
        self.fate
    }

    /// Each graph the edit changes the wiring of, in graph order.
    pub fn graphs(&self) -> impl ExactSizeIterator<Item = (GraphId, &WiringChange)> {
        self.graphs.iter().map(|(graph, change)| (*graph, change))
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
        let mut graphs = BTreeMap::new();
        for (graph, held) in after.graphs() {
            let mut change = WiringChange::default();
            let scope = after.scope(library, graph);
            let was = self.scope(library, graph);
            let was_graph = self.graph(graph);
            // The name and type a port of a node the graph holds had, where the new interface no
            // longer gives it.
            let lost_input = |port: InputPort| {
                if scope
                    .decl(held.find(port.node_id)?)
                    .ports()
                    .input(port.port)
                    .is_some()
                {
                    return None;
                }
                let input = was
                    .decl(was_graph.find(port.node_id)?)
                    .ports()
                    .input(port.port)?;
                Some(SavedPort {
                    name: input.name.clone(),
                    data_type: input.data_type,
                })
            };
            let lost_output = |port: OutputPort| {
                if scope
                    .decl(held.find(port.node_id)?)
                    .ports()
                    .output(port.port)
                    .is_some()
                {
                    return None;
                }
                let output = was
                    .decl(was_graph.find(port.node_id)?)
                    .ports()
                    .output(port.port)?;
                Some(SavedPort {
                    name: output.name.clone(),
                    data_type: output.data_type,
                })
            };
            let lost_event = |event: EventPort| {
                if scope
                    .decl(held.find(event.node_id)?)
                    .event(event.event)
                    .is_some()
                {
                    return None;
                }
                let old = was
                    .decl(was_graph.find(event.node_id)?)
                    .event(event.event)?;
                Some(SavedEvent {
                    name: old.name.clone(),
                })
            };
            let lost_trigger = |trigger: TriggerPort| {
                let node = held.find(trigger.node_id)?;
                if scope.decl(node).triggers().contains(trigger.trigger) {
                    return None;
                }
                let old = match was.decl(was_graph.find(trigger.node_id)?).triggers() {
                    NodeTriggers::PerEvent(events) => {
                        events.iter().find(|event| event.id == trigger.trigger)?
                    }
                    NodeTriggers::None | NodeTriggers::Own => return None,
                };
                Some(SavedEvent {
                    name: old.name.clone(),
                })
            };
            for refusal in held.refused_wiring(scope) {
                let port = match refusal {
                    RefusedWiring::Binding { port, .. } => port,
                    RefusedWiring::Subscription { subscription, .. } => {
                        change.refused.subscriptions.insert(
                            subscription,
                            GhostSubscription {
                                event: lost_event(subscription.event),
                                trigger: lost_trigger(subscription.trigger),
                            },
                        );
                        continue;
                    }
                };
                let source = match held.wiring.bindings[&port].clone() {
                    Binding::Const(value) => GhostSource::Const(value),
                    Binding::Wire(output) => GhostSource::Wire {
                        output,
                        saved: lost_output(output),
                    },
                };
                change.refused.bindings.insert(
                    port,
                    GhostBinding {
                        input: lost_input(port),
                        source,
                    },
                );
            }
            // A ghost already kept whose end names a port the edit removes takes the name the
            // old interface gave it, as a func's remap names one (`port_map`): its row then reads
            // by name, not by number.
            for (port, ghost) in held.ghost_wiring().bindings() {
                let source = match &ghost.source {
                    GhostSource::Wire { output, saved } => GhostSource::Wire {
                        output: *output,
                        saved: saved.clone().or_else(|| lost_output(*output)),
                    },
                    source @ GhostSource::Const(_) => source.clone(),
                };
                let after = GhostBinding {
                    input: ghost.input.clone().or_else(|| lost_input(port)),
                    source,
                };
                if after != *ghost {
                    change.named_before.bindings.insert(port, ghost.clone());
                    change.named_after.bindings.insert(port, after);
                }
            }
            for (subscription, ghost) in held.ghost_wiring().subscriptions() {
                let after = GhostSubscription {
                    event: ghost
                        .event
                        .clone()
                        .or_else(|| lost_event(subscription.event)),
                    trigger: ghost
                        .trigger
                        .clone()
                        .or_else(|| lost_trigger(subscription.trigger)),
                };
                if after != *ghost {
                    change
                        .named_before
                        .subscriptions
                        .insert(subscription, ghost.clone());
                    change.named_after.subscriptions.insert(subscription, after);
                }
            }
            if !change.is_empty() {
                graphs.insert(graph, change);
            }
        }
        InterfaceChange {
            id,
            before,
            after: to,
            fate,
            graphs,
        }
    }

    /// Land `side` of `change`: its interface, with each refused binding taken out, or kept as
    /// a ghost, for [`ChangeSide::After`], and put back live for [`ChangeSide::Before`].
    ///
    /// # Panics
    /// If the set is not on the other side of `change`: an input does not hold what was
    /// recorded.
    pub fn write_interface(&mut self, change: &InterfaceChange, side: ChangeSide) {
        if side == ChangeSide::Before {
            self.set_interface(change.id, change.before.clone());
        }
        for (&graph, wiring) in &change.graphs {
            let held = self.graph_mut(graph);
            for (&port, ghost) in &wiring.refused.bindings {
                let live = InputState::Live(ghost.binding());
                let refused = match change.fate {
                    RefusedFate::Drop => InputState::Unbound,
                    RefusedFate::Ghost => InputState::Ghost(ghost.clone()),
                };
                let [from, to] = match side {
                    ChangeSide::After => [live, refused],
                    ChangeSide::Before => [refused, live],
                };
                assert_eq!(
                    held.input_state(port),
                    from,
                    "an interface edit lands on the input it recorded"
                );
                held.set_input_state(port, to);
            }
            for (&subscription, ghost) in &wiring.refused.subscriptions {
                let refused = match change.fate {
                    RefusedFate::Drop => SubscriptionState::Absent,
                    RefusedFate::Ghost => SubscriptionState::Ghost(ghost.clone()),
                };
                let [from, to] = match side {
                    ChangeSide::After => [SubscriptionState::Live, refused],
                    ChangeSide::Before => [refused, SubscriptionState::Live],
                };
                assert_eq!(
                    held.subscription_state(subscription),
                    from,
                    "an interface edit lands on the subscription it recorded"
                );
                held.set_subscription_state(subscription, to);
            }
            // A ghost the edit names an end of takes the name, and gives it back on the way back.
            let [from, to] = match side {
                ChangeSide::After => [&wiring.named_before, &wiring.named_after],
                ChangeSide::Before => [&wiring.named_after, &wiring.named_before],
            };
            for (&port, ghost) in &to.bindings {
                assert_eq!(
                    held.input_state(port),
                    InputState::Ghost(from.bindings[&port].clone()),
                    "an interface edit names the ghost it recorded"
                );
                held.set_input_state(port, InputState::Ghost(ghost.clone()));
            }
            for (&subscription, ghost) in &to.subscriptions {
                assert_eq!(
                    held.subscription_state(subscription),
                    SubscriptionState::Ghost(from.subscriptions[&subscription].clone()),
                    "an interface edit names the ghost it recorded"
                );
                held.set_subscription_state(subscription, SubscriptionState::Ghost(ghost.clone()));
            }
        }
        if side == ChangeSide::After {
            self.set_interface(change.id, change.after.clone());
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::graph::ghost_wiring::{GhostBinding, GhostSubscription};
    use crate::graph::interface_change::InterfaceChange;
    use crate::graph::subscription::Subscription;
    use crate::identity::{GraphId, InputPort};

    /// One binding an interface edit refuses, as a test reads it.
    #[derive(Debug)]
    pub(crate) struct RefusedBinding<'a> {
        pub(crate) graph: GraphId,
        pub(crate) port: InputPort,
        pub(crate) ghost: &'a GhostBinding,
    }

    /// One subscription an interface edit refuses, as a test reads it.
    #[derive(Debug, PartialEq, Eq)]
    pub(crate) struct RefusedSubscription<'a> {
        pub(crate) graph: GraphId,
        pub(crate) subscription: Subscription,
        pub(crate) ghost: &'a GhostSubscription,
    }

    impl InterfaceChange {
        /// Each binding the edit refuses, graph by graph, then by input.
        pub(crate) fn refused_bindings(&self) -> Vec<RefusedBinding<'_>> {
            self.graphs()
                .flat_map(|(graph, change)| {
                    change
                        .refused()
                        .bindings()
                        .map(move |(port, ghost)| RefusedBinding { graph, port, ghost })
                })
                .collect()
        }

        /// Each subscription the edit refuses, graph by graph, then by subscription.
        pub(crate) fn refused_subscriptions(&self) -> Vec<RefusedSubscription<'_>> {
            self.graphs()
                .flat_map(|(graph, change)| {
                    change
                        .refused()
                        .subscriptions()
                        .map(move |(subscription, ghost)| RefusedSubscription {
                            graph,
                            subscription,
                            ghost,
                        })
                })
                .collect()
        }
    }
}
