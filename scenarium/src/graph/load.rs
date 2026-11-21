//! [`GraphLoad`]: the repair of one graph at a load, each step reading the graph under its scope
//! and writing it in one method.
//!

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

use hashbrown::HashMap;

use crate::decl::saved_decl::{SavedDecl, SavedEvent, SavedInput, SavedPort};
use crate::graph::binding::Binding;
use crate::graph::error::{
    BindRejection, DetachReason, GhostSubscriptionRejection, SubscriptionRejection,
};
use crate::graph::ghost_node::GhostNode;
use crate::graph::ghost_wiring::{
    GhostBinding, GhostSource, GhostSubscription, InputState, SubscriptionState,
};
use crate::graph::manifest::Manifest;
use crate::graph::node::{Boundary, Node};
use crate::graph::node_decl::NodeDecl;
use crate::graph::reconciled::Repair;
use crate::graph::scope::Scope;
use crate::graph::subscription::Subscription;
use crate::graph::wiring::Wiring;
use crate::graph::{Detaching, Graph, RefusedWiring};
use crate::identity::{InputPort, NodeId};
use crate::library::func_redirect::FuncRedirect;

/// What each node of one graph declared before this load: the saved declaration of a ghost node
/// the load attached, else its func's in the file's manifest.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Before<'a> {
    pub(crate) manifest: &'a Manifest,
    /// The saved declaration of each ghost node this load attached, by node.
    pub(crate) returned: &'a HashMap<NodeId, SavedDecl>,
}

impl<'a> Before<'a> {
    fn of(self, node_id: NodeId, node: &Node) -> Option<&'a SavedDecl> {
        self.returned
            .get(&node_id)
            .or_else(|| self.manifest.func(node.declared_func()?))
    }
}

/// A node a rule across graphs refuses, and which.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Refusal {
    pub(crate) node_id: NodeId,
    pub(crate) reason: DetachReason,
}

/// Which ghosts a [`GraphLoad::restore`] makes live of those the scope admits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Restore {
    /// Every one: a load's.
    All,
    /// Those that touch the node, as an edit of one node restores no other.
    Touching(NodeId),
}

/// One graph of a set under repair: the graph, the scope it resolves against, and the repairs
/// each step reports.
#[derive(Debug)]
pub(crate) struct GraphLoad<'a> {
    pub(crate) graph: &'a mut Graph,
    pub(crate) scope: Scope<'a>,
    pub(crate) repairs: &'a mut Vec<Repair>,
}

/// The constants a load drops as the default their port declared, and the saved port each other
/// ghost holds for each end: what [`GraphLoad::note_ghosts`] reads before it writes.
#[derive(Debug, Default)]
struct GhostNotes {
    defaults: Vec<InputPort>,
    bindings: Vec<BindingNote>,
    subscriptions: Vec<SubscriptionNote>,
}

#[derive(Debug)]
struct BindingNote {
    port: InputPort,
    input: Option<SavedPort>,
    output: Option<SavedPort>,
}

#[derive(Debug)]
struct SubscriptionNote {
    subscription: Subscription,
    event: Option<SavedEvent>,
    trigger: Option<SavedEvent>,
}

impl GraphLoad<'_> {
    /// Repair what no declaration decides: a node under the nil id, a ghost node under a live
    /// node's id, and wiring that names a node the graph holds as neither, or a ghost the live
    /// table still holds the key of.
    ///
    /// Live wiring first, so a ghost is weighed against the live wiring that stays: a live
    /// binding that names a missing node goes, and a ghost on the same input stays, as a load
    /// drops no wiring whose two nodes the graph keeps.
    pub(crate) fn repair_structure(&mut self) {
        let graph = &mut *self.graph;
        let repairs = &mut *self.repairs;
        if graph.nodes.remove(&NodeId::nil()).is_some() {
            repairs.push(Repair::NilNode);
        }
        if graph.ghost_nodes.remove(&NodeId::nil()).is_some() {
            repairs.push(Repair::NilNode);
        }
        let nodes = &graph.nodes;
        graph.ghost_nodes.retain(|node_id, _| {
            let kept = !nodes.contains_key(node_id);
            if !kept {
                repairs.push(Repair::DuplicateGhostNode { node_id: *node_id });
            }
            kept
        });
        let ghost_nodes = &graph.ghost_nodes;
        let held =
            |node_id: NodeId| nodes.contains_key(&node_id) || ghost_nodes.contains_key(&node_id);

        let ghost_wiring = &mut graph.ghost_wiring;
        graph.wiring.bindings.retain(|port, binding| {
            let ends = [
                Some(port.node_id),
                match binding {
                    Binding::Wire(source) => Some(source.node_id),
                    Binding::Const(_) => None,
                },
            ];
            let Some(off) = ends
                .into_iter()
                .flatten()
                .find(|node_id| !nodes.contains_key(node_id))
            else {
                return true;
            };
            if ends.into_iter().flatten().all(held) {
                // A ghost of the same binding keeps the ports it saved; one of another goes, as
                // an input holds one binding.
                match ghost_wiring.bindings.entry(*port) {
                    Entry::Occupied(saved) if saved.get().binding() == *binding => {}
                    Entry::Occupied(mut saved) => {
                        repairs.push(Repair::GhostOnBoundInput {
                            port: *port,
                            binding: saved.get().binding(),
                        });
                        saved.insert(GhostBinding::from(binding.clone()));
                    }
                    Entry::Vacant(free) => {
                        free.insert(GhostBinding::from(binding.clone()));
                    }
                }
                repairs.push(Repair::GhostedBinding {
                    port: *port,
                    binding: binding.clone(),
                    rejection: BindRejection::GhostNode { node_id: off },
                });
            } else {
                repairs.push(Repair::DanglingBinding {
                    port: *port,
                    binding: binding.clone(),
                });
            }
            false
        });
        graph.wiring.subscriptions.retain(|subscription| {
            let ends = [subscription.event.node_id, subscription.trigger.node_id];
            let Some(off) = ends
                .into_iter()
                .find(|node_id| !nodes.contains_key(node_id))
            else {
                return true;
            };
            if ends.into_iter().all(held) {
                // A ghost of the same subscription keeps the events it saved.
                ghost_wiring.subscriptions.entry(*subscription).or_default();
                repairs.push(Repair::GhostedSubscription {
                    subscription: *subscription,
                    rejection: GhostSubscriptionRejection::GhostNode { node_id: off },
                });
            } else {
                repairs.push(Repair::DanglingSubscription {
                    subscription: *subscription,
                });
            }
            false
        });

        let bindings = &graph.wiring.bindings;
        graph.ghost_wiring.bindings.retain(|port, ghost| {
            let ends_held =
                held(port.node_id) && ghost.output().is_none_or(|output| held(output.node_id));
            if !ends_held {
                repairs.push(Repair::DanglingBinding {
                    port: *port,
                    binding: ghost.binding(),
                });
            } else if bindings.contains_key(port) {
                repairs.push(Repair::GhostOnBoundInput {
                    port: *port,
                    binding: ghost.binding(),
                });
            }
            ends_held && !bindings.contains_key(port)
        });
        let subscriptions = &graph.wiring.subscriptions;
        graph.ghost_wiring.subscriptions.retain(|subscription, _| {
            let ends_held = held(subscription.event.node_id) && held(subscription.trigger.node_id);
            if !ends_held {
                repairs.push(Repair::DanglingSubscription {
                    subscription: *subscription,
                });
            } else if subscriptions.contains(subscription) {
                repairs.push(Repair::GhostOfSubscription {
                    subscription: *subscription,
                });
            }
            ends_held && !subscriptions.contains(subscription)
        });
    }

    /// Move each node and ghost node of a func the scope's library redirects to the func its
    /// redirects end at, in node-id order, its wiring through their remap; a ghost node's saved
    /// declaration, else `manifest`, names the ports of the func it was. Report each.
    pub(crate) fn redirect(&mut self, manifest: &Manifest) {
        let library = self.scope.library();
        let graph = &mut *self.graph;
        let repairs = &mut *self.repairs;
        let nodes = graph.iter().map(|node| (node.node_id, node.func_id()));
        let ghosts = graph
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
            let before = graph
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
            graph.redirect(node_id, redirect.to, &redirect.remap, &before, &after);
            repairs.push(Repair::Redirected {
                node_id,
                from: redirect.from,
                to: redirect.to,
            });
        }
    }

    /// Attach each ghost node the scope resolves, in node-id order, the node alone, and report
    /// each. Its wiring stays among the ghosts until [`Self::restore`] finds the graph can hold
    /// it. Hands back each ghost node as it was, for what it declared before.
    pub(crate) fn reattach(&mut self) -> BTreeMap<NodeId, GhostNode> {
        let scope = self.scope;
        let reattached: BTreeMap<NodeId, GhostNode> = self
            .graph
            .ghost_nodes
            .extract_if(.., |_, ghost| scope.resolve(&ghost.node).is_ok())
            .collect();
        for (&node_id, ghost) in &reattached {
            self.graph.insert(node_id, ghost.node.clone());
            self.repairs.push(Repair::Reattached { node_id });
        }
        reattached
    }

    /// Add the boundary node a compound's graph lacks on each side, under a fresh id; the main
    /// graph has none.
    pub(crate) fn add_missing_boundaries(&mut self) {
        if self.scope.owner().is_none() {
            return;
        }
        let graph = &mut *self.graph;
        let repairs = &mut *self.repairs;
        for boundary in [Boundary::Inputs, Boundary::Outputs] {
            if graph.boundary_nodes(boundary).next().is_none() {
                let node_id = graph.add(Node::from(boundary));
                repairs.push(Repair::BoundaryAdded { boundary, node_id });
            }
        }
    }

    /// Make a ghost node of each node the scope does not let this graph hold, in node-id order:
    /// each node it does not resolve, and in a compound's graph each `Outputs` node past the
    /// lowest id. Each keeps what `manifest` says its func declared.
    pub(crate) fn detach_unresolved(&mut self, manifest: &Manifest) {
        let scope = self.scope;
        let graph = &*self.graph;
        let mut refused: Vec<Refusal> = graph
            .nodes
            .iter()
            .filter_map(|(node_id, node)| {
                Some(Refusal {
                    node_id: *node_id,
                    reason: DetachReason::from(scope.resolve(node).err()?),
                })
            })
            .collect();
        if scope.owner().is_some() {
            let held = graph.boundary_nodes(Boundary::Outputs);
            refused.extend(held.skip(1).map(|node_id| Refusal {
                node_id,
                reason: DetachReason::ExtraOutputs,
            }));
        }
        refused.sort_unstable_by_key(|refusal| refusal.node_id);
        self.detach(manifest, refused);
    }

    /// Make a ghost node of each node of `refused`, with what it declared: its declaration
    /// where it resolves, as a node refused for a rule across graphs does, else its func's
    /// saved one in `manifest`.
    ///
    /// # Panics
    /// If the graph holds a node of `refused` as no node.
    pub(crate) fn detach(&mut self, manifest: &Manifest, refused: Vec<Refusal>) {
        let scope = self.scope;
        let detaching = refused
            .into_iter()
            .map(|Refusal { node_id, reason }| {
                let node = self
                    .graph
                    .find(node_id)
                    .expect("a node to detach is in the graph");
                let saved = match scope.resolve(node) {
                    Ok(decl) => Some(decl.saved()),
                    Err(_) => node
                        .declared_func()
                        .and_then(|func_id| manifest.func(func_id))
                        .cloned(),
                };
                Detaching {
                    node_id,
                    reason,
                    saved,
                }
            })
            .collect();
        self.graph.detach_into_ghosts(detaching, self.repairs);
    }

    /// Make a ghost of each binding and subscription the scope refuses, then of each wire that
    /// closes a data cycle, and report each. A subscription no library can bring back
    /// ([`Graph::impossible`]) is removed.
    pub(crate) fn ghost_refused(&mut self) {
        let refused = self.graph.refused_wiring(self.scope);
        let graph = &mut *self.graph;
        let repairs = &mut *self.repairs;
        for refusal in refused {
            match refusal {
                RefusedWiring::Binding { port, rejection } => {
                    graph.ghost_binding(port, BindRejection::Binding(rejection), repairs);
                }
                RefusedWiring::Subscription {
                    subscription,
                    rejection,
                } => {
                    graph.wiring.subscriptions.remove(&subscription);
                    repairs.push(match rejection {
                        // A declaration that loses the event or the trigger may regain it.
                        SubscriptionRejection::UndeclaredEvent
                        | SubscriptionRejection::UndeclaredTrigger => {
                            graph
                                .ghost_wiring
                                .subscriptions
                                .insert(subscription, GhostSubscription::default());
                            Repair::GhostedSubscription {
                                subscription,
                                rejection: GhostSubscriptionRejection::Subscription(rejection),
                            }
                        }
                        SubscriptionRejection::SelfSubscription
                        | SubscriptionRejection::ImpossibleTrigger => {
                            Repair::RejectedSubscription {
                                subscription,
                                rejection,
                            }
                        }
                    });
                }
            }
        }
        for port in graph.closing_wires() {
            graph.ghost_binding(port, BindRejection::Cycle, repairs);
        }
    }

    /// Take out each ghost subscription no library can let this graph hold
    /// ([`Graph::impossible`]), and report each: a file can hold one, and no ghost of it ever
    /// comes back.
    pub(crate) fn drop_impossible_ghosts(&mut self) {
        let graph = &mut *self.graph;
        let repairs = &mut *self.repairs;
        let impossible: Vec<_> = graph
            .ghost_wiring
            .subscriptions()
            .filter_map(|(subscription, _)| {
                graph
                    .impossible(subscription)
                    .map(|rejection| (subscription, rejection))
            })
            .collect();
        for (subscription, rejection) in impossible {
            graph.ghost_wiring.subscriptions.remove(&subscription);
            repairs.push(Repair::RejectedSubscription {
                subscription,
                rejection,
            });
        }
    }

    /// Make each ghost of `which` the scope admits live, the loop check aside, each in the order
    /// the graph keeps it, a wire only while it closes no data cycle among the wires live before
    /// it, and report each.
    pub(crate) fn restore(&mut self, which: Restore) {
        let scope = self.scope;
        let graph = &*self.graph;
        let wanted_binding = |port: InputPort| match which {
            Restore::All => true,
            Restore::Touching(node_id) => port.node_id == node_id,
        };
        let wanted_subscription = |subscription: Subscription| match which {
            Restore::All => true,
            Restore::Touching(node_id) => subscription.touches(node_id),
        };
        let bindings: Vec<InputPort> = graph
            .ghost_wiring
            .bindings()
            .filter(|(port, _)| wanted_binding(*port))
            .filter(|(port, ghost)| graph.admit_kept(scope, *port, &ghost.binding()).is_ok())
            .map(|(port, _)| port)
            .collect();
        let subscriptions: Vec<Subscription> = graph
            .ghost_wiring
            .subscriptions()
            .filter(|(subscription, _)| wanted_subscription(*subscription))
            .filter(|(subscription, _)| {
                graph.admit_ghost_subscription(scope, *subscription).is_ok()
            })
            .map(|(subscription, _)| subscription)
            .collect();
        let graph = &mut *self.graph;
        for port in bindings {
            let ghost = &graph.ghost_wiring.bindings[&port];
            if ghost
                .output()
                .is_some_and(|output| graph.produces_cycle(output.node_id, port.node_id))
            {
                continue;
            }
            let binding = ghost.binding();
            graph.set_input_state(port, InputState::Live(binding.clone()));
            self.repairs.push(Repair::RestoredBinding { port, binding });
        }
        for subscription in subscriptions {
            graph.set_subscription_state(subscription, SubscriptionState::Live);
            self.repairs
                .push(Repair::RestoredSubscription { subscription });
        }
    }

    /// Settle the ghosts the graph still keeps against what each node declared `before`: a
    /// constant on an input of a node of the graph that is the default the input declared goes,
    /// as nobody set it, and is reported; each other ghost holds the saved port of each end no
    /// declaration the graph can read gives, a node's or a ghost node's saved one, taken from
    /// `before` where the ghost has none yet. Hands back the inputs whose default went, in port
    /// order.
    pub(crate) fn note_ghosts(&mut self, before: Before<'_>) -> Vec<InputPort> {
        let scope = self.scope;
        let notes = {
            let graph = &*self.graph;
            let mut notes = GhostNotes::default();
            for (port, ghost) in graph.ghost_wiring.bindings() {
                let consumer = graph.nodes.get(&port.node_id);
                let before_input = consumer
                    .and_then(|node| before.of(port.node_id, node))
                    .and_then(|decl| decl.input(port.port));
                if let (GhostSource::Const(value), Some(declared)) = (&ghost.source, before_input)
                    && declared.default_value.as_ref() == Some(value)
                {
                    notes.defaults.push(port);
                    continue;
                }
                let input = if graph.shown_input(scope, port, None).is_some() {
                    None
                } else {
                    ghost
                        .input
                        .clone()
                        .or_else(|| before_input.map(SavedInput::port))
                };
                let output = match &ghost.source {
                    GhostSource::Const(_) => None,
                    GhostSource::Wire { output, saved } => {
                        if graph.shown_output(scope, *output, None).is_some() {
                            None
                        } else {
                            saved.clone().or_else(|| {
                                let node = graph.nodes.get(&output.node_id)?;
                                before
                                    .of(output.node_id, node)?
                                    .output(output.port)
                                    .cloned()
                            })
                        }
                    }
                };
                let held = match &ghost.source {
                    GhostSource::Const(_) => None,
                    GhostSource::Wire { saved, .. } => saved.as_ref(),
                };
                if input != ghost.input || output.as_ref() != held {
                    notes.bindings.push(BindingNote {
                        port,
                        input,
                        output,
                    });
                }
            }
            for (subscription, ghost) in graph.ghost_wiring.subscriptions() {
                let event_port = subscription.event;
                let event = if graph.shown_event(scope, event_port, None).is_some() {
                    None
                } else {
                    ghost.event.clone().or_else(|| {
                        let node = graph.nodes.get(&event_port.node_id)?;
                        before
                            .of(event_port.node_id, node)?
                            .event(event_port.event)
                            .cloned()
                    })
                };
                // A trigger's saved name is the interface event it passed out, which no file's
                // declaration knows: it goes once the node shows the trigger again.
                let trigger = graph
                    .nodes
                    .get(&subscription.trigger.node_id)
                    .is_none_or(|node| {
                        !scope
                            .decl(node)
                            .triggers()
                            .contains(subscription.trigger.trigger)
                    })
                    .then(|| ghost.trigger.clone())
                    .flatten();
                if event != ghost.event || trigger != ghost.trigger {
                    notes.subscriptions.push(SubscriptionNote {
                        subscription,
                        event,
                        trigger,
                    });
                }
            }
            notes
        };
        let graph = &mut *self.graph;
        let repairs = &mut *self.repairs;
        for &port in &notes.defaults {
            let ghost = graph
                .ghost_wiring
                .bindings
                .remove(&port)
                .expect("a noted ghost is kept");
            let GhostSource::Const(value) = ghost.source else {
                unreachable!("a default is a constant");
            };
            repairs.push(Repair::DefaultDropped { port, value });
        }
        for note in &notes.bindings {
            let ghost = graph
                .ghost_wiring
                .bindings
                .get_mut(&note.port)
                .expect("a noted ghost is kept");
            ghost.input.clone_from(&note.input);
            if let GhostSource::Wire { saved, .. } = &mut ghost.source {
                saved.clone_from(&note.output);
            }
        }
        for note in &notes.subscriptions {
            let ghost = graph
                .ghost_wiring
                .subscriptions
                .get_mut(&note.subscription)
                .expect("a noted ghost is kept");
            ghost.event.clone_from(&note.event);
            ghost.trigger.clone_from(&note.trigger);
        }
        notes.defaults
    }

    /// Bind each input of the graph's nodes that holds nothing to its default, where its node did
    /// not declare the port `before` or [`Self::note_ghosts`] dropped the default as a ghost, in
    /// `dropped`, and report each. A node `before` knows nothing of takes none, as nothing says
    /// which of its ports are new.
    pub(crate) fn seed_defaults(&mut self, before: Before<'_>, dropped: &[InputPort]) {
        debug_assert!(dropped.is_sorted(), "dropped defaults come in port order");
        let scope = self.scope;
        let seeds = {
            let graph = &*self.graph;
            let mut seeds = Wiring::default();
            for node in graph.iter() {
                let Some(declared_before) = before.of(node.node_id, &node) else {
                    continue;
                };
                for input in scope.decl(&node).inputs() {
                    let port = InputPort::new(node.node_id, input.id);
                    let gained = declared_before.input(input.id).is_none();
                    if (gained || dropped.binary_search(&port).is_ok())
                        && graph.input_state(port) == InputState::Unbound
                        && let Some(default) = &input.default_value
                    {
                        seeds.bind(port, Binding::Const(default.clone()));
                    }
                }
            }
            seeds
        };
        let graph = &mut *self.graph;
        let repairs = &mut *self.repairs;
        for (port, binding) in seeds.bindings {
            repairs.push(Repair::DefaultSeeded {
                port,
                binding: binding.clone(),
            });
            graph.set_input_state(port, InputState::Live(binding));
        }
    }
}
