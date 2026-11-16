//! [`InterfaceChange`]: the reversible record of an edit of a subgraph's interface.
//!
//! # Decisions
//!
//! - **An interface edit is the set's, and it changes another graph only by the wiring it
//!   drops.** A reorder and a rename change no binding anywhere, as wiring names a port by id. A
//!   removed port takes the wires and constants on it in every graph, a retyped one those it no
//!   longer admits, and undo puts them back: the record is the two interfaces and those
//!   bindings. Refused: the interface as a field a host sets, which leaves the dropped wires in
//!   other graphs to each host.
//! - **The plan reads a copy of the set under the new interface**, so the one admission rule
//!   decides what goes. Cost: one copy of the set's graphs per interface edit, a person's action.
//!   Refused: a second rule that reads the types beside [`Graph::admit`](crate::Graph::admit),
//!   which would drift from it.

use serde::{Deserialize, Serialize};

use crate::graph::RefusedWiring;
use crate::graph::binding::Binding;
use crate::graph::graph_set::GraphSet;
use crate::graph::identity::{GraphId, InputPort, SubgraphId};
use crate::graph::interface::Interface;
use crate::library::Library;

/// An edit of one subgraph's interface: the interface before and after, and each binding the
/// new interface does not hold, in any graph. [`GraphSet::write_interface`] lands either side.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterfaceChange {
    id: SubgraphId,
    before: Interface,
    after: Interface,
    dropped: Vec<DroppedBinding>,
}

/// A binding an interface edit takes out: on an instance, from an instance's output, or on or
/// from a boundary node inside.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DroppedBinding {
    pub graph: GraphId,
    pub port: InputPort,
    pub binding: Binding,
}

/// Which side of an [`InterfaceChange`] a write lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeSide {
    Before,
    After,
}

impl InterfaceChange {
    pub const fn id(&self) -> SubgraphId {
        self.id
    }

    pub const fn before(&self) -> &Interface {
        &self.before
    }

    pub const fn after(&self) -> &Interface {
        &self.after
    }

    /// In graph order, then in the order each graph keeps its bindings.
    pub fn dropped(&self) -> &[DroppedBinding] {
        &self.dropped
    }
}

impl GraphSet {
    /// What setting subgraph `id`'s interface to `to` changes: each binding, in any graph, that
    /// the new interface does not hold — on or from an instance of `id` whose port it lacks or no
    /// longer admits, and on or from a boundary node of `id`'s graph likewise.
    ///
    /// # Panics
    /// If this set holds no subgraph under `id`.
    pub fn plan_interface(
        &self,
        id: SubgraphId,
        to: Interface,
        library: &Library,
    ) -> InterfaceChange {
        let before = self
            .subgraph(id)
            .expect("an interface edit's subgraph is in the set")
            .interface
            .clone();
        let mut after = self.clone();
        after.set_interface(id, to.clone());
        let mut dropped = Vec::new();
        for (graph, held) in after.graphs() {
            for refusal in held.refused_wiring(after.scope(library, graph)) {
                let RefusedWiring::Binding { port, .. } = refusal else {
                    unreachable!(
                        "an interface has no event ports, so its edit refuses no subscription"
                    );
                };
                dropped.push(DroppedBinding {
                    graph,
                    port,
                    binding: held.bindings[&port].clone(),
                });
            }
        }
        InterfaceChange {
            id,
            before,
            after: to,
            dropped,
        }
    }

    /// Land `side` of `change`: its interface, with each dropped binding taken out for
    /// [`ChangeSide::After`] and put back for [`ChangeSide::Before`].
    ///
    /// # Panics
    /// If the set is not on the other side of `change`: a dropped binding is not as recorded, or
    /// its input is bound where it goes back.
    pub fn write_interface(&mut self, change: &InterfaceChange, side: ChangeSide) {
        match side {
            ChangeSide::After => {
                for dropped in &change.dropped {
                    let held = self.graph_mut(dropped.graph).bindings.remove(&dropped.port);
                    assert_eq!(
                        held.as_ref(),
                        Some(&dropped.binding),
                        "an interface edit drops the binding it recorded"
                    );
                }
                self.set_interface(change.id, change.after.clone());
            }
            ChangeSide::Before => {
                self.set_interface(change.id, change.before.clone());
                for dropped in &change.dropped {
                    let prev = self
                        .graph_mut(dropped.graph)
                        .bindings
                        .insert(dropped.port, dropped.binding.clone());
                    assert!(
                        prev.is_none(),
                        "an undone interface edit restores an unbound input"
                    );
                }
            }
        }
    }
}
