//! The move of a node's wiring through a [`NodeRemap`]: what a replace and a redirect do.
//!
//! # Decisions
//!
//! - **What a map does not name is kept as a ghost**, under the name and type the old
//!   declaration gave it. A port id is a key only within one declaration, so an unnamed port
//!   whose id the new declaration gives another port moves to a minted id the new one does not
//!   give: else a later load would make it live on a port it never meant.

use std::collections::BTreeMap;

use crate::decl::port_map::{NodeRemap, PortMap};
use crate::decl::saved_decl::{SavedDecl, SavedInput};
use crate::graph::Graph;
use crate::graph::binding::Binding;
use crate::graph::detached::{DetachedGhost, DetachedNode, DetachedNodeParts};
use crate::graph::ghost_node::GhostNode;
use crate::graph::ghost_wiring::{GhostBinding, GhostSource, GhostSubscription, GhostWiring};
use crate::graph::manifest::Manifest;
use crate::graph::node::Node;
use crate::graph::node_decl::NodeDecl;
use crate::graph::reconciled::Repair;
use crate::graph::scope::Scope;
use crate::graph::subscription::Subscription;
use crate::graph::wiring::Wiring;
use crate::identity::FuncId;
use crate::identity::{EventPort, InputPort, NodeId, OutputPort, PortId};
use crate::library::Library;
use crate::library::func_redirect::FuncRedirect;

/// Where one old port of a node lands: on a new port, its wiring live, or on a ghost port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Landing {
    Port(PortId),
    Ghost(PortId),
}

impl Landing {
    const fn id(self) -> PortId {
        match self {
            Self::Port(id) | Self::Ghost(id) => id,
        }
    }
}

/// Each old port of one side, as [`NodeRemap`] and the new declaration place it.
#[derive(Debug)]
struct Side(BTreeMap<PortId, Landing>);

impl Side {
    /// Place each of `olds` through `map`: a named port on its new one; an unnamed one on a ghost
    /// port under its own id, or a minted one where `declared`, the new side, or the map gives
    /// that id to another port.
    fn of(map: &PortMap, declared: &[PortId], olds: impl IntoIterator<Item = PortId>) -> Self {
        let mut landings: BTreeMap<PortId, Landing> = BTreeMap::new();
        let mut minted: Vec<PortId> = Vec::new();
        for old in olds {
            if landings.contains_key(&old) {
                continue;
            }
            let landing = match map.get(old) {
                Some(new) => Landing::Port(new),
                None if declared.contains(&old) || map.targets().any(|new| new == old) => {
                    minted.push(old);
                    continue;
                }
                None => Landing::Ghost(old),
            };
            landings.insert(old, landing);
        }
        for old in minted {
            let id = PortId::mint(|id| {
                declared.contains(&id)
                    || map.targets().any(|new| new == id)
                    || landings.values().any(|landing| landing.id() == id)
                    || landings.contains_key(&id)
            });
            landings.insert(old, Landing::Ghost(id));
        }
        Self(landings)
    }

    fn get(&self, old: PortId) -> Landing {
        self.0[&old]
    }
}

/// The three sides of one node's remap.
#[derive(Debug)]
struct Sides {
    inputs: Side,
    outputs: Side,
    events: Side,
}

impl Sides {
    /// The sides of `node_id`'s ports that `wiring` and `ghosts` name, placed through `remap`
    /// against `after`.
    fn of(
        node_id: NodeId,
        remap: &NodeRemap,
        after: &SavedDecl,
        wiring: &Wiring,
        ghosts: &GhostWiring,
    ) -> Self {
        let declared_inputs: Vec<PortId> = after.inputs.iter().map(|input| input.id).collect();
        let declared_outputs: Vec<PortId> = after.outputs.iter().map(|output| output.id).collect();
        let declared_events: Vec<PortId> = after.events.iter().map(|event| event.id).collect();
        let live_inputs = wiring.bindings_on(node_id).map(|(port, _)| port.port);
        let ghost_inputs = ghosts.bindings_on(node_id).map(|(port, _)| port.port);
        let live_outputs = wiring
            .wires()
            .filter(|(_, output)| output.node_id == node_id)
            .map(|(_, output)| output.port);
        let ghost_outputs = ghosts
            .wires()
            .filter(|(_, output)| output.node_id == node_id)
            .map(|(_, output)| output.port);
        let live_events = wiring
            .subscriptions_from(node_id)
            .map(|subscription| subscription.event.event);
        let ghost_events = ghosts
            .subscriptions_from(node_id)
            .map(|(subscription, _)| subscription.event.event);
        Self {
            inputs: Side::of(
                &remap.inputs,
                &declared_inputs,
                live_inputs.chain(ghost_inputs),
            ),
            outputs: Side::of(
                &remap.outputs,
                &declared_outputs,
                live_outputs.chain(ghost_outputs),
            ),
            events: Side::of(
                &remap.events,
                &declared_events,
                live_events.chain(ghost_events),
            ),
        }
    }
}

/// What a remap makes of one node's wiring: the live wiring that lands on new ports, and the
/// ghosts.
#[derive(Debug, Default)]
struct Remapped {
    wiring: Wiring,
    ghosts: GhostWiring,
}

impl Remapped {
    /// `node_id`'s wiring moved through `sides`; `before` names a port a ghost lands on.
    fn of(
        node_id: NodeId,
        sides: &Sides,
        before: &SavedDecl,
        wiring: Wiring,
        ghosts: GhostWiring,
    ) -> Self {
        let mut out = Self::default();
        let input_port = |port: InputPort| -> (InputPort, Option<Landing>) {
            if port.node_id != node_id {
                return (port, None);
            }
            let landing = sides.inputs.get(port.port);
            (InputPort::new(node_id, landing.id()), Some(landing))
        };
        let output_port = |output: OutputPort| -> (OutputPort, Option<Landing>) {
            if output.node_id != node_id {
                return (output, None);
            }
            let landing = sides.outputs.get(output.port);
            (OutputPort::new(node_id, landing.id()), Some(landing))
        };
        let saved_input = |old: InputPort| {
            (old.node_id == node_id)
                .then(|| before.input(old.port).map(SavedInput::port))
                .flatten()
        };
        let saved_output = |old: OutputPort| {
            (old.node_id == node_id)
                .then(|| before.output(old.port).cloned())
                .flatten()
        };
        for (port, binding) in wiring.bindings {
            let (input, input_landing) = input_port(port);
            let ghost_input = matches!(input_landing, Some(Landing::Ghost(_)));
            match binding {
                Binding::Const(value) if ghost_input => {
                    out.ghosts.bindings.insert(
                        input,
                        GhostBinding {
                            input: saved_input(port),
                            source: GhostSource::Const(value),
                        },
                    );
                }
                Binding::Const(value) => {
                    out.wiring.bind(input, Binding::Const(value));
                }
                Binding::Wire(output) => {
                    let (source, output_landing) = output_port(output);
                    let ghost_output = matches!(output_landing, Some(Landing::Ghost(_)));
                    if ghost_input || ghost_output {
                        out.ghosts.bindings.insert(
                            input,
                            GhostBinding {
                                input: if ghost_input { saved_input(port) } else { None },
                                source: GhostSource::Wire {
                                    output: source,
                                    saved: if ghost_output {
                                        saved_output(output)
                                    } else {
                                        None
                                    },
                                },
                            },
                        );
                    } else {
                        out.wiring.bind(input, Binding::Wire(source));
                    }
                }
            }
        }
        for (port, ghost) in ghosts.bindings {
            let (input, input_landing) = input_port(port);
            let saved_in = match input_landing {
                Some(Landing::Ghost(_)) => ghost.input.or_else(|| saved_input(port)),
                Some(Landing::Port(_)) | None => ghost.input,
            };
            let source = match ghost.source {
                GhostSource::Const(value) => GhostSource::Const(value),
                GhostSource::Wire { output, saved } => {
                    let (source, landing) = output_port(output);
                    let saved = match landing {
                        Some(Landing::Ghost(_)) => saved.or_else(|| saved_output(output)),
                        Some(Landing::Port(_)) | None => saved,
                    };
                    GhostSource::Wire {
                        output: source,
                        saved,
                    }
                }
            };
            out.ghosts.bindings.insert(
                input,
                GhostBinding {
                    input: saved_in,
                    source,
                },
            );
        }
        let event_of = |subscription: Subscription| -> (Subscription, Option<Landing>) {
            if subscription.event.node_id != node_id {
                return (subscription, None);
            }
            let landing = sides.events.get(subscription.event.event);
            (
                Subscription::new(EventPort::new(node_id, landing.id()), subscription.trigger),
                Some(landing),
            )
        };
        let saved_event = |old: Subscription| before.event(old.event.event).cloned();
        for subscription in wiring.subscriptions {
            match event_of(subscription) {
                (moved, Some(Landing::Ghost(_))) => {
                    out.ghosts.subscriptions.insert(
                        moved,
                        GhostSubscription {
                            event: saved_event(subscription),
                            trigger: None,
                        },
                    );
                }
                (moved, _) => {
                    out.wiring.subscribe(moved);
                }
            }
        }
        for (subscription, ghost) in ghosts.subscriptions {
            let (moved, landing) = event_of(subscription);
            let event = match landing {
                Some(Landing::Ghost(_)) => ghost.event.or_else(|| saved_event(subscription)),
                Some(Landing::Port(_)) | None => ghost.event,
            };
            out.ghosts
                .subscriptions
                .insert(moved, GhostSubscription { event, ..ghost });
        }
        out
    }
}

impl DetachedNode {
    /// This record as `node`, a node of the declaration `after`, its wiring moved through
    /// `remap` from the declaration `before`: what the map names lands on the new ports, live as
    /// it was; what it does not becomes a ghost with `before`'s name and type. The wiring the new
    /// declaration refuses for its type is left for the graph to ghost when it takes the record.
    #[must_use]
    pub fn remapped(
        self,
        node: Node,
        remap: &NodeRemap,
        before: &SavedDecl,
        after: &SavedDecl,
    ) -> Self {
        let Self {
            node_id,
            node: _,
            wiring,
            ghost_wiring,
        } = self;
        let sides = Sides::of(node_id, remap, after, &wiring, &ghost_wiring);
        let out = Remapped::of(node_id, &sides, before, wiring, ghost_wiring);
        Self::new(DetachedNodeParts {
            node_id,
            node,
            wiring: out.wiring,
            ghost_wiring: out.ghosts,
        })
        .expect("a remap moves a well-formed record's wiring to distinct ports of its node")
    }
}

impl DetachedGhost {
    /// This ghost node as `ghost`, its ghosts moved through `remap` from `before` to `after`, as
    /// [`DetachedNode::remapped`] moves a node's: a ghost stays a ghost, on its new port.
    pub(crate) fn remapped(
        self,
        ghost: GhostNode,
        remap: &NodeRemap,
        before: &SavedDecl,
        after: &SavedDecl,
    ) -> Self {
        let Self {
            node_id, wiring, ..
        } = self;
        let sides = Sides::of(node_id, remap, after, &Wiring::default(), &wiring);
        let out = Remapped::of(node_id, &sides, before, Wiring::default(), wiring);
        Self {
            node_id,
            ghost,
            wiring: out.ghosts,
        }
    }
}

impl Graph {
    /// `node_id`, a node or a ghost node, as a node of func `to`, its wiring moved through
    /// `remap` from `before` to `after`: what a load does with a node of a func the library
    /// redirects. A ghost node stays one, now of `to` and holding `after`, for the load's next
    /// steps to attach; the wiring the new declaration refuses is theirs to ghost too.
    ///
    /// # Panics
    /// If the graph holds neither a node nor a ghost node `node_id`, or it runs no func.
    pub(crate) fn redirect(
        &mut self,
        node_id: NodeId,
        to: FuncId,
        remap: &NodeRemap,
        before: &SavedDecl,
        after: &SavedDecl,
    ) {
        if self.find(node_id).is_some() {
            let record = self.detach_node(node_id);
            let node = record.node().redirected(to);
            self.attach_node(record.remapped(node, remap, before, after));
            return;
        }
        let record = self.detach_ghost_node(node_id);
        let ghost = GhostNode {
            node: record.ghost().node.redirected(to),
            reason: record.ghost().reason,
            saved: Some(after.clone()),
        };
        self.attach_ghost_node(record.remapped(ghost, remap, before, after));
    }

    /// Move each node and ghost node of a func `library` redirects to the func its redirects end
    /// at, in node-id order, its wiring through their remap; a ghost node's saved declaration, else
    /// `manifest`, names the ports of the func it was. Report each.
    pub(crate) fn redirect_all(
        &mut self,
        library: &Library,
        manifest: &Manifest,
        repairs: &mut Vec<Repair>,
    ) {
        let nodes = self.iter().map(|node| (node.node_id, node.func_id()));
        let ghosts = self
            .ghost_nodes()
            .map(|(node_id, ghost)| (node_id, ghost.node.func_id()));
        let mut redirected: Vec<(NodeId, &FuncRedirect)> = nodes
            .chain(ghosts)
            .filter_map(|(node_id, func)| Some((node_id, library.redirect(func?)?)))
            .collect();
        redirected.sort_unstable_by_key(|(node_id, _)| *node_id);
        for (node_id, redirect) in redirected {
            // A ghost node's func was missing at its save, so the file listed none for it: it
            // holds what its func declared itself.
            let before = self
                .ghost_node(node_id)
                .and_then(|ghost| ghost.saved.clone())
                .or_else(|| manifest.func(redirect.from).cloned())
                .unwrap_or_default();
            let after = NodeDecl::Func(
                library
                    .by_id(redirect.to)
                    .expect("a redirect ends at a registered func"),
            )
            .saved();
            self.redirect(node_id, redirect.to, &redirect.remap, &before, &after);
            repairs.push(Repair::Redirected {
                node_id,
                from: redirect.from,
                to: redirect.to,
            });
        }
    }

    /// The record `node_id`, a node or a ghost node, would be as `with`, its wiring moved
    /// through `remap`, read under `scope`: what the map names lands on the new ports, live
    /// where the new declaration takes it and a ghost where it does not; what the map does not
    /// name is a ghost under the name and type the old declaration gave it; and a ghost the new
    /// declaration takes comes back live. What Replace puts in place of the node it takes out.
    ///
    /// # Panics
    /// If the graph holds neither a node nor a ghost node `node_id`, or `scope` does not
    /// resolve `with`.
    pub fn replaced(
        &self,
        scope: Scope<'_>,
        node_id: NodeId,
        with: Node,
        remap: &NodeRemap,
    ) -> DetachedNode {
        let after = scope
            .resolve(&with)
            .expect("a node a replace puts in resolves")
            .saved();
        let mut trial = self.clone();
        let record = if let Some(node) = self.find(node_id) {
            let before = scope.decl(node).saved();
            trial
                .detach_node(node_id)
                .remapped(with, remap, &before, &after)
        } else {
            let ghost = trial.detach_ghost_node(node_id);
            let before = ghost.ghost().saved.clone().unwrap_or_default();
            DetachedNode::new(DetachedNodeParts {
                node_id,
                node: with.clone(),
                wiring: Wiring::default(),
                ghost_wiring: ghost.wiring().clone(),
            })
            .expect("a ghost node's ghosts all touch it")
            .remapped(with, remap, &before, &after)
        };
        trial.attach_node(record);
        let mut scratch = Vec::new();
        let refused = trial.refused_wiring(scope);
        trial.ghost_refused(&refused, &mut scratch);
        let mut admitted = trial.admitted_ghosts(scope);
        admitted.retain_touching(node_id);
        trial.restore(&admitted, &mut scratch);
        trial
            .snapshot_node(node_id)
            .expect("the replaced node is in the trial")
    }
}

#[cfg(test)]
mod tests;
