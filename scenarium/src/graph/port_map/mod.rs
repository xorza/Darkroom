//! [`PortMap`] and [`NodeRemap`]: where each port of one declaration went in another, and the
//! move of a node's wiring through them.
//!
//! # Decisions
//!
//! - **One remap moves a node from one declaration to another**, for a func a person replaces
//!   and for a func the library redirects: three maps, inputs, outputs and events, each a partial
//!   injection from old port ids to new ones. Nothing pairs by name here: a name is a guess a
//!   person confirms before a remap is made, never one a load acts on.
//! - **What a map does not name is kept as a ghost**, under the name and type the old
//!   declaration gave it. A port id is a key only within one declaration, so an unnamed port
//!   whose id the new declaration gives another port moves to a minted id the new one does not
//!   give: else a later load would make it live on a port it never meant.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::graph::Graph;
use crate::graph::binding::Binding;
use crate::graph::binding_entry::BindingEntry;
use crate::graph::detached::{DetachedGhost, DetachedNode, DetachedNodeParts};
use crate::graph::ghost_node::GhostNode;
use crate::graph::ghost_wiring::{GhostBinding, GhostSource, GhostSubscription, GhostWiring};
use crate::graph::identity::FuncId;
use crate::graph::identity::{EventPort, InputPort, NodeId, OutputPort, PortId};
use crate::graph::manifest::Manifest;
use crate::graph::node::Node;
use crate::graph::node_decl::NodeDecl;
use crate::graph::reconciled::{Reconciled, Repair};
use crate::graph::saved_decl::{SavedDecl, SavedInput};
use crate::graph::scope::Scope;
use crate::graph::subscription::Subscription;
use crate::library::Library;
use crate::library::func_redirect::FuncRedirect;

/// Old port ids to new ones, each side at most once. Built by [`Self::checked`] alone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<(PortId, PortId)>", into = "Vec<(PortId, PortId)>")]
pub struct PortMap {
    /// Sorted by old id.
    pairs: Vec<(PortId, PortId)>,
}

/// Why [`PortMap::checked`] refuses a map.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum PortMapError {
    #[error("old port {0} maps twice")]
    OldTwice(PortId),
    #[error("new port {0} is the target of two old ports")]
    NewTwice(PortId),
}

impl PortMap {
    /// The map of `pairs`, old to new; refused where an old or a new id appears twice.
    pub fn checked(
        pairs: impl IntoIterator<Item = (PortId, PortId)>,
    ) -> Result<Self, PortMapError> {
        let mut pairs: Vec<(PortId, PortId)> = pairs.into_iter().collect();
        pairs.sort_unstable();
        if let Some(pair) = pairs.windows(2).find(|pair| pair[0].0 == pair[1].0) {
            return Err(PortMapError::OldTwice(pair[0].0));
        }
        let mut targets: Vec<PortId> = pairs.iter().map(|(_, new)| *new).collect();
        targets.sort_unstable();
        if let Some(pair) = targets.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(PortMapError::NewTwice(pair[0]));
        }
        Ok(Self { pairs })
    }

    /// [`Self::checked`], panicking on its error.
    pub fn new(pairs: impl IntoIterator<Item = (PortId, PortId)>) -> Self {
        Self::checked(pairs).expect("a port map is an injection")
    }

    /// Where old port `old` went, if the map names it.
    pub fn get(&self, old: PortId) -> Option<PortId> {
        self.pairs
            .binary_search_by_key(&old, |(from, _)| *from)
            .ok()
            .map(|idx| self.pairs[idx].1)
    }

    /// In old-id order.
    pub fn pairs(&self) -> &[(PortId, PortId)] {
        &self.pairs
    }

    fn targets(&self) -> impl Iterator<Item = PortId> + '_ {
        self.pairs.iter().map(|(_, new)| *new)
    }

    /// This map followed by `next`: each old port to where `next` takes the port this map takes
    /// it to, dropped where `next` names none.
    #[must_use]
    pub fn then(&self, next: &Self) -> Self {
        Self::new(
            self.pairs
                .iter()
                .filter_map(|&(old, mid)| Some((old, next.get(mid)?))),
        )
    }

    /// The pairs a person would expect from `before` to `after`: a port to the new one of the
    /// same id and type, else to an unpaired one of the same name and type. `same` says whether
    /// two ports agree in type.
    fn propose(
        before: &[(PortId, &str)],
        after: &[(PortId, &str)],
        same: impl Fn(PortId, PortId) -> bool,
    ) -> Self {
        let mut pairs: Vec<(PortId, PortId)> = Vec::new();
        let taken =
            |pairs: &[(PortId, PortId)], new: PortId| pairs.iter().any(|(_, to)| *to == new);
        for &(old, _) in before {
            if after.iter().any(|(new, _)| *new == old) && same(old, old) {
                pairs.push((old, old));
            }
        }
        for &(old, name) in before {
            if pairs.iter().any(|(from, _)| *from == old) {
                continue;
            }
            if let Some(&(new, _)) = after
                .iter()
                .find(|&&(new, to)| to == name && same(old, new) && !taken(&pairs, new))
            {
                pairs.push((old, new));
            }
        }
        Self::new(pairs)
    }
}

impl TryFrom<Vec<(PortId, PortId)>> for PortMap {
    type Error = PortMapError;

    fn try_from(pairs: Vec<(PortId, PortId)>) -> Result<Self, PortMapError> {
        Self::checked(pairs)
    }
}

impl From<PortMap> for Vec<(PortId, PortId)> {
    fn from(map: PortMap) -> Self {
        map.pairs
    }
}

/// Where each port of a node's old declaration went in its new one: its inputs, its outputs and
/// its events.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRemap {
    pub inputs: PortMap,
    pub outputs: PortMap,
    pub events: PortMap,
}

impl NodeRemap {
    /// The remap a person would expect from the declaration `before` to `after`: each port to
    /// the new one of its id and type, else to an unpaired one of its name and type; an event, which
    /// has no type, by its id, else its name. A proposal a person confirms before it is made.
    pub fn propose(before: &SavedDecl, after: &SavedDecl) -> Self {
        let [inputs_before, inputs_after] = [before, after].map(|decl| {
            decl.inputs
                .iter()
                .map(|input| (input.id, input.name.as_str()))
                .collect::<Vec<_>>()
        });
        let [outputs_before, outputs_after] = [before, after].map(|decl| {
            decl.outputs
                .iter()
                .map(|output| (output.id, output.name.as_str()))
                .collect::<Vec<_>>()
        });
        let [events_before, events_after] = [before, after].map(|decl| {
            decl.events
                .iter()
                .map(|event| (event.id, event.name.as_str()))
                .collect::<Vec<_>>()
        });
        let same_input = |old: PortId, new: PortId| {
            before.input(old).map(|input| input.data_type)
                == after.input(new).map(|input| input.data_type)
        };
        let same_output = |old: PortId, new: PortId| {
            before.output(old).map(|output| output.data_type)
                == after.output(new).map(|output| output.data_type)
        };
        Self {
            inputs: PortMap::propose(&inputs_before, &inputs_after, same_input),
            outputs: PortMap::propose(&outputs_before, &outputs_after, same_output),
            events: PortMap::propose(&events_before, &events_after, |_, _| true),
        }
    }

    /// This remap followed by `next`, side by side.
    #[must_use]
    pub fn then(&self, next: &Self) -> Self {
        Self {
            inputs: self.inputs.then(&next.inputs),
            outputs: self.outputs.then(&next.outputs),
            events: self.events.then(&next.events),
        }
    }
}

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
    /// The sides of `node_id`'s ports that `bindings`, `subscriptions` and `ghosts` name, placed
    /// through `remap` against `after`.
    fn of(
        node_id: NodeId,
        remap: &NodeRemap,
        after: &SavedDecl,
        bindings: &[BindingEntry],
        subscriptions: &[Subscription],
        ghosts: &GhostWiring,
    ) -> Self {
        let ids = |ports: &mut dyn Iterator<Item = PortId>| ports.collect::<Vec<_>>();
        let declared_inputs = ids(&mut after.inputs.iter().map(|input| input.id));
        let declared_outputs = ids(&mut after.outputs.iter().map(|output| output.id));
        let declared_events = ids(&mut after.events.iter().map(|event| event.id));
        let live_inputs = bindings
            .iter()
            .filter(|entry| entry.port.node_id == node_id)
            .map(|entry| entry.port.port);
        let ghost_inputs = ghosts.bindings_on(node_id).map(|(port, _)| port.port);
        let live_outputs = bindings.iter().filter_map(|entry| match entry.binding {
            Binding::Wire(output) if output.node_id == node_id => Some(output.port),
            Binding::Wire(_) | Binding::Const(_) => None,
        });
        let ghost_outputs = ghosts
            .wires()
            .filter(|(_, output)| output.node_id == node_id)
            .map(|(_, output)| output.port);
        let live_events = subscriptions
            .iter()
            .filter(|subscription| subscription.event.node_id == node_id)
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
    bindings: Vec<BindingEntry>,
    subscriptions: Vec<Subscription>,
    ghosts: GhostWiring,
}

impl Remapped {
    /// `node_id`'s wiring moved through `sides`; `before` names a port a ghost lands on.
    fn of(
        node_id: NodeId,
        sides: &Sides,
        before: &SavedDecl,
        bindings: Vec<BindingEntry>,
        subscriptions: Vec<Subscription>,
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
        for BindingEntry { port, binding } in bindings {
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
                Binding::Const(value) => out.bindings.push(BindingEntry {
                    port: input,
                    binding: Binding::Const(value),
                }),
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
                        out.bindings.push(BindingEntry {
                            port: input,
                            binding: Binding::Wire(source),
                        });
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
        for subscription in subscriptions {
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
                (moved, _) => out.subscriptions.push(moved),
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
            bindings,
            subscriptions,
            ghost_wiring,
        } = self;
        let sides = Sides::of(
            node_id,
            remap,
            after,
            &bindings,
            &subscriptions,
            &ghost_wiring,
        );
        let out = Remapped::of(
            node_id,
            &sides,
            before,
            bindings,
            subscriptions,
            ghost_wiring,
        );
        Self::new(DetachedNodeParts {
            node_id,
            node,
            bindings: out.bindings,
            subscriptions: out.subscriptions,
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
        let sides = Sides::of(node_id, remap, after, &[], &[], &wiring);
        let out = Remapped::of(node_id, &sides, before, Vec::new(), Vec::new(), wiring);
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
        reconciled: &mut Reconciled,
    ) {
        let nodes = self.iter().map(|node| (node.node_id, node.func_id()));
        let ghosts = self
            .ghost_nodes()
            .map(|(node_id, ghost)| (node_id, ghost.node.func_id()));
        let mut redirected: Vec<(NodeId, FuncRedirect)> = nodes
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
            reconciled.repairs.push(Repair::Redirected {
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
            let before = node.decl(scope).saved();
            trial
                .detach_node(node_id)
                .remapped(with, remap, &before, &after)
        } else {
            let ghost = trial.detach_ghost_node(node_id);
            let before = ghost.ghost().saved.clone().unwrap_or_default();
            DetachedNode::new(DetachedNodeParts {
                node_id,
                node: with.clone(),
                bindings: Vec::new(),
                subscriptions: Vec::new(),
                ghost_wiring: ghost.wiring().clone(),
            })
            .expect("a ghost node's ghosts all touch it")
            .remapped(with, remap, &before, &after)
        };
        trial.attach_node(record);
        let mut scratch = Reconciled::default();
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
