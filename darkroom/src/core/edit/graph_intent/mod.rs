//! What a widget wants the graph to look like, and how that lands.
//!
//! A [`GraphIntent`] is forward-only — "set X to Y", no history — and is the
//! vocabulary every surface of the editor speaks. Turning one into something
//! reversible is [`GraphIntent::into_step`]: it reads the state the intent
//! overwrites out of a `&Document` and folds the two into an
//! [`UndoStep`].
//!
//! The two vocabularies are deliberately *not* parallel. Intents are named
//! for what a user did, steps for what the document can take back, and the
//! lowering between them is free to expand one into several (duplicating a
//! selection) or to send two opposite intents to one step (adding and
//! removing a node). Adding an intent is a variant plus an arm here; adding a
//! step is a file in [`step`](crate::core::edit::step).
//!
//! The same split runs the other way, by *scope*: a `GraphIntent` edits the
//! graph, while a [`DockOperation`](palantir::DockOperation) edits the
//! layout around it. Neither can be mistaken for the other, so no code path
//! has to carry state it will not read.

pub(crate) mod click_mode;
pub(crate) mod drag_start;
pub(crate) mod incoming_wires;
pub(crate) mod route_insert;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::iter;
use std::sync::Arc;

use glam::Vec2;
use scenarium::{
    Binding, BindingEntry, DetachedNode, DetachedNodeParts, EventPort, Graph as CoreGraph,
    InputPort, Library, Node, NodeId, OutputPort, Subscription,
};

use crate::core::document::Document;
use crate::core::document::graph_view::{GraphView, NodePlacement};
use crate::core::document::item_id::ItemId;
use crate::core::document::route::Route;
use crate::core::document::tie::{Tie, TieAngle, TieId};
use crate::core::document::viewport::Viewport;
use crate::core::document::wire_key::{DataWire, WireKey};
use crate::core::edit::error::{MalformedIntent, Role};
use crate::core::edit::gesture_id::GestureId;
use crate::core::edit::graph_intent::click_mode::ClickMode;
use crate::core::edit::graph_intent::drag_start::DragStart;
use crate::core::edit::graph_intent::incoming_wires::IncomingWires;
use crate::core::edit::graph_intent::route_insert::RouteInsert;
use crate::core::edit::step::change::Change;
use crate::core::edit::step::set_depth::SetDepth;
use crate::core::edit::step::set_input::SetInput;
use crate::core::edit::step::set_node::{NodeEntry, SetNode};
use crate::core::edit::step::set_node_name::SetNodeName;
use crate::core::edit::step::set_node_property::{NodeProperty, SetNodeProperty};
use crate::core::edit::step::set_positions::{Move, SetPositions};
use crate::core::edit::step::set_selection::SetSelection;
use crate::core::edit::step::set_subscription::SetSubscription;
use crate::core::edit::step::set_ties::TiesDraft;
use crate::core::edit::step::set_viewport::SetViewport;
use crate::core::edit::step::undo_step::UndoStep;
use crate::core::edit::unwired;
use crate::core::edit::validate;
use crate::core::edit::validate::Insertion;

/// World-space offset applied to duplicated nodes so the copies don't land
/// exactly on top of their originals.
const DUPLICATE_OFFSET: Vec2 = Vec2::new(32.0, 32.0);

/// What the caller wants to change in the document's graph. Forward-only — no
/// `from` fields: each variant says "set X to Y", and the previous Y is
/// captured at commit time by [`Self::into_step`].
///
/// Every variant here edits the graph, and travels the frame's queue as its
/// `Graph` tier. A mutation of the layout instead of the graph is a
/// [`DockOperation`](palantir::DockOperation), which travels the same
/// queue in the `View` tier.
#[derive(Debug)]
pub(crate) enum GraphIntent {
    /// Add one node that links state the document already resolves: a func in
    /// the library, or a built-in special.
    AddNode {
        /// Where the node lands on the canvas; its placement is created
        /// alongside it, at the top of the paint stack.
        pos: Vec2,
        node_id: NodeId,
        node: Node,
        /// Initial input bindings to seed alongside the node — the caller
        /// fills these with each input's func-declared default
        /// (`Binding::Const`) so a fresh node lands ready to run instead of
        /// fully unbound. Applied atomically with the node, so one undo
        /// removes node + seeds together. They must be the new node's own
        /// inputs; wiring *into* an existing node is a separate
        /// [`Self::SetInput`].
        bindings: Vec<BindingEntry>,
    },
    RemoveNode {
        node_id: NodeId,
    },
    /// One frame of a drag of one or more node bodies and ties, in canvas-world
    /// coordinates: every member of `members` moves to its start plus
    /// `offset`. A multi-select drag moves the whole group; a plain drag
    /// carries just the grabbed item. The frames of one `gesture` fold into
    /// one undo entry.
    MoveSelection {
        gesture: GestureId,
        /// Shared with the drag that latched them, so a frame copies no list.
        members: Arc<[DragStart]>,
        offset: Vec2,
    },
    RenameNode {
        node_id: NodeId,
        to: String,
    },
    SetInput {
        input: InputPort,
        to: Option<Binding>,
    },
    /// Replace the whole selection set. The rubber band, node/pin clicks,
    /// and Esc-deselect all funnel through this — the caller computes
    /// the desired final set and the undo layer captures the prior one.
    SetSelection {
        to: BTreeSet<ItemId>,
    },
    /// Lift a node — its body or a pinned output's preview widget — to
    /// the top of its graph's paint stack, by setting its
    /// [`NodePlacement::z`] past every other. Emitted when either kind is
    /// clicked or grabbed, so clicking brings it forward. The depth is stored
    /// view state, so it persists across save/load and tab switches and walks
    /// with undo/redo.
    Raise {
        node_id: NodeId,
    },
    /// Set one scalar property of a node — its `disabled` flag or its cache
    /// mode. See [`NodeProperty`].
    SetNodeProperty {
        node_id: NodeId,
        to: NodeProperty,
    },
    /// Move the graph camera. A frame of a held gesture names it, so its
    /// frames fold into one undo entry; a one-shot jump names none.
    SetViewport {
        to: Viewport,
        gesture: Option<GestureId>,
    },
    /// Add (`subscribe = true`) or remove (`false`) an event subscription: an
    /// event wire dropped on, or severed from, a subscription pin.
    /// Idempotent — a no-op when the subscription already matches.
    SetSubscription {
        subscription: Subscription,
        subscribe: bool,
    },
    /// Gather `wires` into a new tie at `pos`, which turns itself: what a tie
    /// line across wires leaves.
    TieWires {
        tie: TieId,
        pos: Vec2,
        wires: Vec<RouteInsert>,
    },
    /// Add `wires` to `tie`: a tie line that also crossed the tie.
    JoinTie {
        tie: TieId,
        wires: Vec<RouteInsert>,
    },
    /// Remove a tie: every wire through it leaves it.
    RemoveTie {
        tie: TieId,
    },
    /// Turn a tie's band. A frame of a held knob drag names its gesture, so
    /// its frames fold into one undo entry; a one-shot turn names none.
    SetTieAngle {
        tie: TieId,
        angle: TieAngle,
        gesture: Option<GestureId>,
    },
    /// Take one wire out of a tie; a tie left with no wire goes with it.
    ReleaseWire {
        tie: TieId,
        wire: WireKey,
    },
}

impl GraphIntent {
    /// The intents a click on `item` produces: the selection change, plus, for
    /// a node, a lift to the top of the paint stack so a clicked node comes to
    /// the front. The raise is skipped when a toggling click *removes* `item`
    /// from the selection — a node you just deselected shouldn't jump
    /// forward. Shared by the node body, its header title and its port
    /// labels, so clicking any of them behaves like clicking the body.
    ///
    /// Takes the committed selection rather than a UI context: that is the
    /// only thing either rule reads, and it keeps the click policy beside the
    /// intents it builds instead of in the widget that raises them.
    pub(crate) fn click(
        mode: ClickMode,
        selected: &BTreeSet<ItemId>,
        item: ItemId,
    ) -> impl Iterator<Item = Self> {
        let deselecting = mode == ClickMode::Toggle && selected.contains(&item);
        let raise = item.node().filter(|_| !deselecting);
        iter::once(Self::select_click(mode, selected, item))
            .chain(raise.map(|node_id| Self::Raise { node_id }))
    }

    /// The [`Self::SetSelection`] a click on `item` produces: a replacing click
    /// selects only it, a toggling one flips its membership. A click that
    /// changes nothing is dropped at commit as a no-op.
    fn select_click(mode: ClickMode, selected: &BTreeSet<ItemId>, item: ItemId) -> Self {
        let mut to = match mode {
            ClickMode::Replace => BTreeSet::new(),
            ClickMode::Toggle => selected.clone(),
        };
        if mode == ClickMode::Toggle && selected.contains(&item) {
            to.remove(&item);
        } else {
            to.insert(item);
        }
        Self::SetSelection { to }
    }

    /// Drop the whole selection. Named rather than spelled `SetSelection`
    /// with an empty set at each call, so a reader sees the intent and not
    /// the mechanism.
    pub(crate) const fn clear_selection() -> Self {
        Self::SetSelection {
            to: BTreeSet::new(),
        }
    }

    /// [`Self::SetInput`] taking a bare binding or `None`, which clears it.
    pub(crate) fn set_input(input: InputPort, to: impl Into<Option<Binding>>) -> Self {
        Self::SetInput {
            input,
            to: to.into(),
        }
    }

    /// Clone `document`'s current selection: each node gets a fresh id and an
    /// offset position, const-value bindings copy verbatim, the data and
    /// event wires *among* the selected nodes are recreated against the
    /// clones with the ties inside them, and the
    /// copies end up selected. A `Wire` whose source is
    /// *outside* the selection goes as `incoming` says: dropped, leaving the
    /// input's declared default as a wire removed on its own does
    /// ([`unwired::binding`]), or kept pointing at the original external
    /// producer. Empty when nothing is selected.
    ///
    /// A list rather than an intent of its own: duplicating is an editor
    /// command built out of the vocabulary, not a graph primitive. The whole
    /// list is queued in one frame, so it commits as a single undo entry —
    /// the same way a swipe that severs five wires does.
    ///
    /// Ordering matters and is the reason the internal edges come out
    /// separately: every clone exists before anything binds to one, so no
    /// intent in the list names a producer that isn't there yet.
    ///
    /// The selection's nodes are the only source of a duplicate set: Ctrl+D and
    /// the node context menu's two Duplicate picks all act on them, the latter
    /// because a right-click selects the node it landed on first. A selected
    /// stub is skipped: a copy of a node that cannot run has little use.
    pub(crate) fn duplicate(
        document: &Document,
        library: &Library,
        incoming: IncomingWires,
    ) -> Vec<Self> {
        let (graph, view) = (&document.graph, &document.main_view);
        // Resolve the whole set first, in selection order: an edge is only
        // recognisable as internal once both of its endpoints have a fresh id,
        // so nothing can be authored until every member has one.
        let mut clones: HashMap<NodeId, NodeId> = HashMap::with_capacity(view.selected.len());
        let mut sources = Vec::with_capacity(view.selected.len());
        for old_id in view.selected.iter().filter_map(|item| item.node()) {
            if document.stubs.contains(old_id) {
                continue;
            }
            let node = graph
                .find(old_id)
                .expect("a selected node is a graph node or a stub");
            let new_id = NodeId::unique();
            clones.insert(old_id, new_id);
            sources.push((old_id, new_id, node));
        }
        let mut intents = Vec::new();
        if sources.is_empty() {
            return intents;
        }

        let mut internal = Vec::new();
        for (old_id, new_id, node) in sources {
            let pos = view
                .node_placements
                .get(&old_id)
                .expect("the view places every node the graph holds")
                .pos
                + DUPLICATE_OFFSET;
            let mut bindings = Vec::new();
            for (port, binding) in graph.input_bindings(old_id) {
                let input = InputPort::new(new_id, port.port_idx);
                let seed = match binding {
                    Binding::Wire(source) => {
                        if let Some(&new_source) = clones.get(&source.node_id) {
                            internal.push(Self::SetInput {
                                input,
                                to: Some(Binding::wire(new_source, source.port_idx)),
                            });
                            continue;
                        }
                        match incoming {
                            IncomingWires::Keep => Some(Binding::Wire(*source)),
                            IncomingWires::Drop => unwired::binding(graph, library, port),
                        }
                    }
                    Binding::Const(_) => Some(binding.clone()),
                };
                bindings.extend(seed.map(|binding| BindingEntry {
                    port: input,
                    binding,
                }));
            }
            intents.push(Self::AddNode {
                pos,
                node_id: new_id,
                node: node.clone(),
                bindings,
            });
        }
        intents.extend(internal);
        for subscription in graph.subscriptions() {
            if let (Some(&emitter), Some(&subscriber)) = (
                clones.get(&subscription.event.node_id),
                clones.get(&subscription.subscriber),
            ) {
                intents.push(Self::SetSubscription {
                    subscription: Subscription::new(
                        EventPort::new(emitter, subscription.event.event_idx),
                        subscriber,
                    ),
                    subscribe: true,
                });
            }
        }
        let tie_clones = Self::duplicate_ties(view, &clones, &mut intents);
        intents.push(Self::SetSelection {
            to: clones
                .values()
                .copied()
                .map(ItemId::Node)
                .chain(tie_clones.into_iter().map(ItemId::Tie))
                .collect(),
        });
        intents
    }

    /// [`Self::duplicate`]'s ties: each tie whose every wire has both end
    /// nodes among `clones`' keys, cloned at the duplicate's offset with its
    /// angle, with each cloned wire's route through the clones in its
    /// original's order. Pushes the intents after every cloned wire's, and
    /// returns the clones' ids.
    fn duplicate_ties(
        view: &GraphView,
        clones: &HashMap<NodeId, NodeId>,
        intents: &mut Vec<Self>,
    ) -> Vec<TieId> {
        let tie_clones: BTreeMap<TieId, TieId> = view
            .ties_within(|node_id| clones.contains_key(&node_id))
            .into_iter()
            .map(|tie| (tie, TieId::unique()))
            .collect();
        // Each cloned wire with the clones on its route, in route order. A
        // tie that also holds a wire leaving the set is not cloned, so its
        // step drops out.
        let routes: Vec<(WireKey, Vec<TieId>)> = view
            .routes
            .iter()
            .filter_map(|(&wire, route)| {
                let steps: Vec<TieId> = route
                    .iter()
                    .filter_map(|tie| tie_clones.get(tie).copied())
                    .collect();
                (!steps.is_empty()).then(|| (cloned_wire(wire, clones), steps))
            })
            .collect();
        let mut made = BTreeSet::new();
        for (original, &tie) in &tie_clones {
            let placed = view.ties[original];
            // A clone joins each route behind the clones already made on it.
            let wires = routes
                .iter()
                .filter_map(|(wire, steps)| {
                    let idx = steps.iter().position(|&step| step == tie)?;
                    Some(RouteInsert {
                        wire: *wire,
                        at: steps[..idx]
                            .iter()
                            .filter(|step| made.contains(*step))
                            .count(),
                    })
                })
                .collect();
            intents.push(Self::TieWires {
                tie,
                pos: placed.pos + DUPLICATE_OFFSET,
                wires,
            });
            if let TieAngle::Pinned(_) = placed.angle {
                intents.push(Self::SetTieAngle {
                    tie,
                    angle: placed.angle,
                    gesture: None,
                });
            }
            made.insert(tie);
        }
        tie_clones.into_values().collect()
    }

    /// What Delete does to `view`'s selection: one removal per member, every
    /// node before every tie, so a frame commits them as one undo entry. A tie
    /// that a node's removal took with it is stale by its turn and gives no
    /// step.
    pub(crate) fn remove_selection(view: &GraphView) -> impl Iterator<Item = Self> + '_ {
        view.selected.iter().map(|&item| match item {
            ItemId::Node(node_id) => Self::RemoveNode { node_id },
            ItemId::Tie(tie) => Self::RemoveTie { tie },
        })
    }

    /// Read the state this intent overwrites out of `document` and fold the two
    /// into a complete [`UndoStep`]. Pure — does not write to the graph.
    ///
    /// This is the only gate between a caller and the document, so each arm
    /// establishes the full precondition set the step's `write` half assumes:
    /// an `Ok` result is a proof that replaying the step trips no assert on
    /// the way and leaves the document passing [`Document::validate`], with a
    /// graph every compile accepts.
    /// Widgets only ever violate the staleness half, since they read the
    /// identities they emit out of the live document; anything else reaching
    /// here is a bug.
    ///
    /// `Ok(None)` covers what a gesture spanning frames does normally: the
    /// anchor node vanished, or the edit is refused by design (a cycle-forming
    /// bind). Callers drop those without a word. A [`MalformedIntent`] means
    /// the payload could never have applied. ([`Self::MoveSelection`] and
    /// [`Self::SetSelection`] instead drop vanished members individually
    /// rather than refusing the whole intent.)
    pub(crate) fn into_step(
        self,
        document: &Document,
        library: &Library,
    ) -> Result<Option<UndoStep>, MalformedIntent> {
        let (graph, view) = (&document.graph, &document.main_view);
        let step = match self {
            Self::AddNode {
                pos,
                node_id,
                node,
                bindings,
            } => {
                validate::fresh_node_id(document, node_id)?;
                validate::finite_pos(pos, Role::AddNode)?;
                let func = validate::resolving_node(library, &node)?;
                validate::seed_bindings(
                    graph,
                    library,
                    Insertion {
                        node_id,
                        func,
                        bindings: &bindings,
                    },
                )?;
                // The depth is fixed here rather than at write time, so a redo
                // puts the node back at the depth the original add gave it
                // instead of jumping it in front of whatever arrived since.
                let placement = NodePlacement {
                    pos,
                    z: view.front_z(),
                };
                let detached = DetachedNode::new(DetachedNodeParts {
                    node_id,
                    node,
                    bindings,
                    subscriptions: Vec::new(),
                })?;
                UndoStep::Node(Box::new(SetNode::insertion(detached, placement)))
            }
            Self::RemoveNode { node_id } => {
                validate::non_nil_node(node_id, Role::RemoveNode)?;
                let Some(entry) = NodeEntry::capture(document, node_id) else {
                    return Ok(None);
                };
                let mut ties = TiesDraft::new(view);
                ties.release(entry.wires());
                UndoStep::Node(Box::new(SetNode::removal(entry, ties.finish())))
            }
            Self::MoveSelection {
                members, offset, ..
            } => {
                let mut moves = Vec::with_capacity(members.len());
                for member in members.iter() {
                    let to = member.pos + offset;
                    validate::finite_pos(to, Role::MoveSelection)?;
                    // Drag-sourced (spans frames): a member whose item
                    // vanished mid-gesture drops quietly.
                    let Some(from) = view.pos(member.item) else {
                        continue;
                    };
                    moves.push(Move {
                        item: member.item,
                        pos: Change { from, to },
                    });
                }
                UndoStep::Positions(SetPositions { moves })
            }
            Self::RenameNode { node_id, to } => {
                let Some(node) = validate::live_node(graph, node_id, Role::RenameNode)? else {
                    return Ok(None);
                };
                UndoStep::NodeName(SetNodeName {
                    node_id,
                    name: Change {
                        from: node.name.clone(),
                        to,
                    },
                })
            }
            Self::SetInput { input, to } => {
                if validate::live_node(graph, input.node_id, Role::SetInputDestination)?.is_none() {
                    return Ok(None);
                }
                let to = match to {
                    None if matches!(graph.bindings.get(&input), Some(Binding::Wire(_))) => {
                        unwired::binding(graph, library, input)
                    }
                    to => to,
                };
                // Only a write is admitted: clearing removes a binding wherever
                // it sits, which is never worse than keeping it.
                if let Some(binding) = &to {
                    if let Binding::Wire(source) = binding {
                        // A wire held across frames can outlive its producer,
                        // and the bind would leave the graph with a dangling
                        // edge.
                        if validate::live_node(graph, source.node_id, Role::SetInputProducer)?
                            .is_none()
                        {
                            return Ok(None);
                        }
                        // A bind that would close a data cycle never lands: a
                        // compile refuses a cyclic graph. The canvas snap
                        // filter normally stops it earlier; this is the guard
                        // covering every binding path.
                        if graph.produces_cycle(source.node_id, input.node_id) {
                            return Ok(None);
                        }
                    }
                    graph.admit(library, input, binding).map_err(|rejection| {
                        MalformedIntent::Rejected {
                            port: input,
                            rejection,
                        }
                    })?;
                }
                let from = graph.bindings.get(&input).cloned();
                let mut ties = TiesDraft::new(view);
                // A wire is both of its ends: one that leaves its output
                // leaves its ties, while a bind to the same output keeps them.
                if let Some(Binding::Wire(output)) = from
                    && !matches!(to, Some(Binding::Wire(source)) if source == output)
                {
                    ties.release([WireKey::Data(DataWire { output, input })]);
                }
                UndoStep::Input(SetInput {
                    input,
                    binding: Change { from, to },
                    ties: ties.finish(),
                })
            }
            Self::SetSelection { to } => UndoStep::Selection(SetSelection {
                selection: Change {
                    from: view.selected.clone(),
                    // The rubber band snapshots identities when the drag
                    // starts, so an interleaved undo can remove one before
                    // release. Keep the members that still have a widget
                    // rather than recording a selection the view can't render.
                    to: to.into_iter().filter(|&item| view.holds(item)).collect(),
                },
            }),
            Self::Raise { node_id } => {
                let (Some(placement), Some(to)) =
                    (view.node_placements.get(&node_id), view.raised_z(node_id))
                else {
                    return Ok(None);
                };
                UndoStep::Depth(SetDepth {
                    node_id,
                    z: Change {
                        from: placement.z,
                        to,
                    },
                })
            }
            Self::SetNodeProperty { node_id, to } => {
                let Some(node) = validate::live_node(graph, node_id, Role::SetNodeProperty)? else {
                    return Ok(None);
                };
                // Capture the *same* property's current value as `from`, so a
                // revert writes a disable flag over a disable flag.
                let from = match to {
                    NodeProperty::Disabled(_) => NodeProperty::Disabled(node.disabled),
                    NodeProperty::RuntimeCache(_) => NodeProperty::RuntimeCache(node.cache),
                };
                UndoStep::NodeProperty(SetNodeProperty {
                    node_id,
                    property: Change { from, to },
                })
            }
            Self::SetViewport { to, .. } => {
                if !to.is_valid() {
                    return Err(MalformedIntent::InvalidViewport);
                }
                UndoStep::Viewport(SetViewport {
                    viewport: Change {
                        from: view.viewport,
                        to,
                    },
                })
            }
            Self::SetSubscription {
                subscription,
                subscribe,
            } => {
                let Subscription { event, subscriber } = subscription;
                validate::non_nil_node(event.node_id, Role::SetSubscriptionEmitter)?;
                validate::non_nil_node(subscriber, Role::SetSubscriptionSubscriber)?;
                // A subscribe needs both endpoints present; a stale drag onto a
                // vanished node drops rather than recording a dangling
                // subscription. An unsubscribe of a vanished node no-ops
                // naturally (nothing is subscribed → from == to == false), so
                // it needs no existence check.
                if subscribe {
                    if graph.find(event.node_id).is_none() || graph.find(subscriber).is_none() {
                        return Ok(None);
                    }
                    graph
                        .admit_subscription(library, subscription)
                        .map_err(|rejection| MalformedIntent::RejectedSubscription {
                            subscription,
                            rejection,
                        })?;
                }
                let from = graph.is_subscribed(event, subscriber);
                let mut ties = TiesDraft::new(view);
                if from && !subscribe {
                    ties.release([WireKey::Event(subscription)]);
                }
                UndoStep::Subscription(SetSubscription {
                    subscription,
                    subscribed: Change {
                        from,
                        to: subscribe,
                    },
                    ties: ties.finish(),
                })
            }
            Self::TieWires { tie, pos, wires } => {
                validate::finite_pos(pos, Role::TieWires)?;
                if view.ties.contains_key(&tie) {
                    return Err(MalformedIntent::TieAlreadyExists { tie });
                }
                if wires.is_empty() {
                    return Err(MalformedIntent::EmptyTie { tie });
                }
                let mut ties = TiesDraft::new(view);
                ties.set_tie(
                    tie,
                    Some(Tie {
                        pos,
                        angle: TieAngle::Auto,
                    }),
                );
                insert_routes(graph, &mut ties, tie, &wires)?;
                UndoStep::Ties(ties.finish())
            }
            // A tie gone since the gesture started is stale: no step.
            Self::JoinTie { tie, wires } => {
                if !view.ties.contains_key(&tie) {
                    return Ok(None);
                }
                let mut ties = TiesDraft::new(view);
                insert_routes(graph, &mut ties, tie, &wires)?;
                UndoStep::Ties(ties.finish())
            }
            Self::RemoveTie { tie } => {
                if !view.ties.contains_key(&tie) {
                    return Ok(None);
                }
                let mut ties = TiesDraft::new(view);
                ties.remove_tie(tie);
                UndoStep::Ties(ties.finish())
            }
            Self::SetTieAngle { tie, angle, .. } => {
                if let TieAngle::Pinned(degrees) = angle
                    && !angle.is_valid()
                {
                    return Err(MalformedIntent::InvalidTieAngle { tie, degrees });
                }
                let Some(&placed) = view.ties.get(&tie) else {
                    return Ok(None);
                };
                let mut ties = TiesDraft::new(view);
                ties.set_tie(tie, Some(Tie { angle, ..placed }));
                UndoStep::Ties(ties.finish())
            }
            // A menu stays open across frames, so the wire may have left the
            // tie since it opened.
            Self::ReleaseWire { tie, wire } => {
                if !view
                    .routes
                    .get(&wire)
                    .is_some_and(|route| route.contains(tie))
                {
                    return Ok(None);
                }
                let mut ties = TiesDraft::new(view);
                ties.leave(wire, tie);
                UndoStep::Ties(ties.finish())
            }
        };
        Ok(Some(step))
    }

    /// The held gesture this intent is a frame of, if any.
    pub(crate) const fn gesture(&self) -> Option<GestureId> {
        match self {
            Self::MoveSelection { gesture, .. } => Some(*gesture),
            Self::SetViewport { gesture, .. } | Self::SetTieAngle { gesture, .. } => *gesture,
            _ => None,
        }
    }

    /// Build, no-op-filter, and apply one intent against `document` in a single
    /// call — the entry every frontend drives its per-intent loop through.
    ///
    /// Pushes each committed [`UndoStep`] onto `steps`, in the order it
    /// applied (the caller records them and reads their signals): none when
    /// nothing came of the intent and nothing was written — a stale anchor, a
    /// cycle-forming bind, or a no-op. A node's removal is followed by a
    /// [`Self::SetInput`] for each input it fed that takes its default back,
    /// as a wire removed on its own leaves it ([`unwired::binding`]). Only a
    /// payload that could never have applied is an `Err` (see
    /// [`Self::into_step`]).
    ///
    /// [`Self::into_step`] and [`UndoStep::apply`] stay separate for the
    /// undo-history redo path, which applies a stored step without rebuilding it
    /// (a redo replays already-valid history).
    pub(crate) fn commit(
        self,
        document: &mut Document,
        library: &Library,
        steps: &mut Vec<UndoStep>,
    ) -> Result<(), MalformedIntent> {
        let fed: Vec<InputPort> = match &self {
            Self::RemoveNode { node_id } => document
                .graph
                .wires()
                .filter(|(_, producer)| producer.node_id == *node_id)
                .map(|(consumer, _)| consumer)
                .collect(),
            _ => Vec::new(),
        };
        let Some(step) = self.into_step(document, library)? else {
            return Ok(());
        };
        if step.is_noop() {
            return Ok(());
        }
        step.apply(document);
        steps.push(step);
        for input in fed {
            if let Some(to) = unwired::binding(&document.graph, library, input) {
                Self::SetInput {
                    input,
                    to: Some(to),
                }
                .commit(document, library, steps)?;
            }
        }
        Ok(())
    }
}

/// `wire` between the clones of its two ends.
///
/// # Panics
///
/// When `clones` holds no clone of an end.
fn cloned_wire(wire: WireKey, clones: &HashMap<NodeId, NodeId>) -> WireKey {
    let clone = |node_id: NodeId| clones[&node_id];
    match wire {
        WireKey::Data(DataWire { output, input }) => WireKey::Data(DataWire {
            output: OutputPort::new(clone(output.node_id), output.port_idx),
            input: InputPort::new(clone(input.node_id), input.port_idx),
        }),
        WireKey::Event(Subscription { event, subscriber }) => WireKey::Event(Subscription::new(
            EventPort::new(clone(event.node_id), event.event_idx),
            clone(subscriber),
        )),
    }
}

/// Put `tie` on each of `wires`' routes, at its crossed piece.
fn insert_routes(
    graph: &CoreGraph,
    ties: &mut TiesDraft<'_>,
    tie: TieId,
    wires: &[RouteInsert],
) -> Result<(), MalformedIntent> {
    for &RouteInsert { wire, at } in wires {
        if !wire.is_held_by(graph) {
            return Err(MalformedIntent::WireAbsent { wire });
        }
        let mut steps: Vec<TieId> = ties
            .route(wire)
            .map(|route| route.iter().copied().collect())
            .unwrap_or_default();
        if steps.contains(&tie) {
            return Err(MalformedIntent::TieAlreadyOnRoute { wire, tie });
        }
        if at > steps.len() {
            return Err(MalformedIntent::RouteIndexOutOfRange {
                wire,
                at,
                len: steps.len(),
            });
        }
        steps.insert(at, tie);
        ties.set_route(wire, Some(Route::new(steps)));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod internals {
    use glam::Vec2;
    use scenarium::{Library, Node, NodeId};

    use crate::core::document::Document;
    use crate::core::edit::error::MalformedIntent;
    use crate::core::edit::graph_intent::GraphIntent;
    use crate::core::edit::step::undo_step::UndoStep;

    impl GraphIntent {
        /// [`Self::commit`] of an intent that commits at most one step.
        pub(crate) fn commit_one(
            self,
            document: &mut Document,
            library: &Library,
        ) -> Result<Option<UndoStep>, MalformedIntent> {
            let mut steps = Vec::new();
            self.commit(document, library, &mut steps)?;
            assert!(steps.len() <= 1, "the intent committed {steps:?}");
            Ok(steps.pop())
        }

        /// An `AddNode` of `node` at `pos` under `node_id`, seeding no binding.
        pub(crate) const fn add_node(pos: Vec2, node_id: NodeId, node: Node) -> Self {
            Self::AddNode {
                pos,
                node_id,
                node,
                bindings: Vec::new(),
            }
        }
    }
}

#[cfg(test)]
mod tests;
