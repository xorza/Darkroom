//! What a widget wants the graph to look like, and how that lands.
//!
//! A [`GraphIntent`] is forward-only ("set X to Y", no history) and is the vocabulary every
//! surface of the editor speaks. Turning one into something reversible is
//! [`GraphIntent::into_step`]: it reads the state the intent overwrites out of a `&Document` and
//! folds the two into an [`UndoStep`].
//!
//! The two vocabularies are deliberately *not* parallel. Intents are named for what a user did,
//! steps for what the document can take back, and the lowering between them may expand one into
//! several (duplicating a selection) or send two opposite intents to one step (adding and
//! removing a node). Adding an intent is a variant plus an arm here; adding a step is a file in
//! [`step`](crate::core::edit::step).
//!
//! The same split runs by *scope*: a `GraphIntent` edits the graph, a
//! [`DockOperation`](palantir::DockOperation) the layout around it.

pub(crate) mod click_mode;
pub(crate) mod drag_start;
pub(crate) mod incoming_wires;
mod node_name;
pub(crate) mod node_property;
pub(crate) mod route_insert;

use std::collections::BTreeSet;
use std::iter;
use std::rc::Rc;

use glam::Vec2;
use scenarium::{
    Binding, BindingEntry, Boundary, DetachedNode, DetachedNodeParts, GhostWiring,
    Graph as CoreGraph, GraphId, InputPort, InputState, Library, Node, NodeId, NodeRemap,
    Subscription, SubscriptionState,
};

use crate::core::document::Document;
use crate::core::document::graph_view::{GraphView, NodePlacement};
use crate::core::document::item_id::ItemId;
use crate::core::document::route::Route;
use crate::core::document::tie::{Tie, TieAngle, TieId};
use crate::core::document::viewport::Viewport;
use crate::core::document::wire_key::{DataWire, WireKey};
use crate::core::edit::error::{EditRefusal, IntentError, MalformedIntent, Role};
use crate::core::edit::fragment::Fragment;
use crate::core::edit::gesture_id::GestureId;
use crate::core::edit::graph_intent::click_mode::ClickMode;
use crate::core::edit::graph_intent::drag_start::DragStart;
use crate::core::edit::graph_intent::incoming_wires::IncomingWires;
use crate::core::edit::graph_intent::node_property::NodeProperty;
use crate::core::edit::graph_intent::route_insert::RouteInsert;
use crate::core::edit::step::change::Change;
use crate::core::edit::step::graph_step::GraphStep;
use crate::core::edit::step::set_depth::SetDepth;
use crate::core::edit::step::set_input::SetInput;
use crate::core::edit::step::set_node::{NodeEntry, SetNode};
use crate::core::edit::step::set_node_name::SetNodeName;
use crate::core::edit::step::set_node_property::{PropertyChange, SetNodeProperty};
use crate::core::edit::step::set_positions::{Move, SetPositions};
use crate::core::edit::step::set_selection::SetSelection;
use crate::core::edit::step::set_subscription::SetSubscription;
use crate::core::edit::step::set_ties::TiesDraft;
use crate::core::edit::step::undo_step::UndoStep;
use crate::core::edit::unwired;
use crate::core::edit::validate;
use crate::core::edit::validate::Insertion;

/// World-space offset of duplicated nodes, so copies don't land on their originals.
const DUPLICATE_OFFSET: Vec2 = Vec2::new(32.0, 32.0);

/// What the caller wants to change in the document's graph. Forward-only, with no `from` fields:
/// the previous value is captured at commit time by [`Self::into_step`].
///
/// Every variant edits the graph and travels the frame's queue as its `Graph` tier; a layout
/// mutation is a [`DockOperation`](palantir::DockOperation) in the `View` tier.
#[derive(Clone, Debug)]
pub(crate) enum GraphIntent {
    /// Add one node that links state the document already resolves: a func in
    /// the library, or a built-in special.
    AddNode {
        /// Where the node lands on the canvas; its placement is created
        /// alongside it, at the top of the paint stack.
        pos: Vec2,
        node_id: NodeId,
        node: Node,
        /// Initial input bindings seeded with the node, usually each input's func-declared
        /// default (`Binding::Const`), so one undo removes node and seeds together. They must be
        /// the new node's own inputs; wiring *into* an existing node is a separate
        /// [`Self::SetInput`].
        bindings: Vec<BindingEntry>,
    },
    RemoveNode {
        node_id: NodeId,
    },
    /// Put `with` in place of the node or ghost node `node_id`, under its id, where it was, its
    /// wiring moved through `remap` ([`Graph::replaced`](scenarium::Graph::replaced)): what the
    /// map names stays live where `with` takes it, the rest is kept as ghosts. One undo entry.
    ReplaceNode {
        node_id: NodeId,
        with: Node,
        remap: NodeRemap,
    },
    /// One frame of a drag of one or more node bodies and ties, in canvas-world coordinates:
    /// every member of `members` moves to its start plus `offset`. The frames of one `gesture`
    /// fold into one undo entry.
    MoveSelection {
        gesture: GestureId,
        /// Shared with the drag that latched them, so a frame copies no list.
        members: Rc<[DragStart]>,
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
    /// Replace the whole selection set: the caller computes the final set and the undo layer
    /// captures the prior one.
    SetSelection {
        to: BTreeSet<ItemId>,
    },
    /// Lift a node, its body or a pinned output's preview widget, to the top of its graph's paint
    /// stack, by setting its [`NodePlacement::z`] past every other. The depth is stored view
    /// state, so it persists across save/load and walks with undo/redo.
    Raise {
        node_id: NodeId,
    },
    /// Set one scalar property of a node: its `disabled` flag or its cache mode.
    SetNodeProperty {
        node_id: NodeId,
        to: NodeProperty,
    },
    /// Move the graph camera. Written straight into the document, it records
    /// no undo step: `darkroom/AGENTS.md`, "Undo", "The camera is outside the
    /// undo history".
    SetViewport {
        to: Viewport,
    },
    /// Add (`to = true`) or remove (`false`) an event subscription. Idempotent: a no-op when the
    /// subscription already matches.
    SetSubscription {
        subscription: Subscription,
        to: bool,
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
    /// The intents a click on `item` produces: the selection change, plus, for a node, a raise to
    /// the front. The raise is skipped when a toggling click *removes* `item` from the selection,
    /// as a node just deselected shouldn't jump forward.
    ///
    /// Takes the committed selection rather than a UI context, which keeps the click policy
    /// beside the intents it builds instead of in the widget that raises them.
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

    /// Drop the whole selection.
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

    /// What Select Ghosts selects in `graph`: each ghost node, and each node a ghost touches.
    pub(crate) fn select_ghosts(graph: &CoreGraph) -> Self {
        let wiring = graph.ghost_wiring();
        let mut to: BTreeSet<ItemId> = graph
            .ghost_nodes()
            .map(|(node_id, _)| ItemId::Node(node_id))
            .collect();
        for (input, ghost) in wiring.bindings() {
            to.insert(ItemId::Node(input.node_id));
            if let Some(output) = ghost.output() {
                to.insert(ItemId::Node(output.node_id));
            }
        }
        for (subscription, _) in wiring.subscriptions() {
            to.insert(ItemId::Node(subscription.event.node_id));
            to.insert(ItemId::Node(subscription.listener.node_id()));
        }
        Self::SetSelection { to }
    }

    /// What Remove Ghosts does to `graph`, committed as one undo entry: each ghost node goes with
    /// the ghosts that touch it, and each other ghost binding and subscription is cleared.
    pub(crate) fn remove_ghosts(graph: &CoreGraph) -> Vec<Self> {
        let ghost_node = |node_id: NodeId| graph.ghost_node(node_id).is_some();
        let wiring = graph.ghost_wiring();
        let nodes = graph
            .ghost_nodes()
            .map(|(node_id, _)| Self::RemoveNode { node_id });
        let bindings = wiring
            .bindings()
            .filter(|(input, ghost)| {
                !ghost_node(input.node_id)
                    && !ghost
                        .output()
                        .is_some_and(|output| ghost_node(output.node_id))
            })
            .map(|(input, _)| Self::set_input(input, None));
        let subscriptions = wiring
            .subscriptions()
            .filter(|(subscription, _)| {
                !ghost_node(subscription.event.node_id)
                    && !ghost_node(subscription.listener.node_id())
            })
            .map(|(subscription, _)| Self::SetSubscription {
                subscription,
                to: false,
            });
        nodes.chain(bindings).chain(subscriptions).collect()
    }

    /// Clone the selection of `graph` of `document`: each node gets a fresh id and an offset
    /// position, const-value bindings copy verbatim, the data and event wires *among* the
    /// selected nodes are recreated against the clones with the ties inside them, and the copies
    /// end up selected. A wire whose source is *outside* the selection goes as `incoming` says
    /// ([`Fragment::capture`]). Each clone and tie clone takes its id from `fresh`, a random one in
    /// the editor, a counter in a test. Empty when no node is selected; a selected ghost node is
    /// skipped.
    ///
    /// A list rather than an intent of its own: duplicating is an editor command built out of the
    /// vocabulary, a [`Fragment`] captured, remapped and landed. The whole list is queued in one
    /// frame, so it commits as one undo entry.
    pub(crate) fn duplicate(
        document: &Document,
        graph: GraphId,
        library: &Library,
        incoming: IncomingWires,
        fresh: impl FnMut() -> u128,
    ) -> Vec<Self> {
        let selected = &document.view(graph).selected;
        Fragment::capture(document, graph, library, selected, incoming)
            .remap(fresh)
            .into_intents(DUPLICATE_OFFSET)
    }

    /// What Delete does to `view`'s selection: one removal per member, every node before every
    /// tie, committed as one undo entry. A tie a node's removal took with it is stale by its turn
    /// and gives no step.
    pub(crate) fn remove_selection(view: &GraphView) -> impl Iterator<Item = Self> + '_ {
        view.selected.iter().map(|&item| match item {
            ItemId::Node(node_id) => Self::RemoveNode { node_id },
            ItemId::Tie(tie) => Self::RemoveTie { tie },
        })
    }

    /// Read the state this intent overwrites out of `graph` of `document` and fold the two into a
    /// complete [`GraphStep`]. Pure: does not write to the document.
    ///
    /// This is the only gate between a caller and the document, so each arm establishes the full
    /// precondition set the step's `write` half assumes: an `Ok` result proves replaying the step
    /// trips no assert and leaves the document passing [`Document::validate`], with a graph every
    /// compile accepts. Widgets only violate the staleness half; anything else is a bug.
    ///
    /// `Ok(None)` covers what a gesture spanning frames does normally: the anchor node vanished,
    /// or the edit is refused by design (a cycle-forming bind). A [`MalformedIntent`] means the
    /// payload could never have applied. ([`Self::MoveSelection`] and [`Self::SetSelection`]
    /// instead drop vanished members individually.)
    fn into_step(
        self,
        document: &Document,
        graph_id: GraphId,
        library: &Library,
    ) -> Result<Option<GraphStep>, IntentError> {
        let (graph, view) = (document.graphs.graph(graph_id), document.view(graph_id));
        let scope = document.graphs.scope(library, graph_id);
        let step = match self {
            Self::AddNode {
                pos,
                node_id,
                node,
                bindings,
            } => {
                validate::fresh_node_id(graph, node_id)?;
                validate::finite_pos(pos, Role::AddNode)?;
                let decl = validate::resolving_node(scope, &node)?;
                // A second `Outputs` node, which would give an interface output two producers.
                if node.boundary() == Some(Boundary::Outputs)
                    && graph
                        .iter()
                        .any(|held| held.boundary() == Some(Boundary::Outputs))
                {
                    return Err(EditRefusal::BoundaryNode.into());
                }
                // An instance that places a subgraph inside itself, or one that takes a graph past
                // a bound.
                document
                    .graphs
                    .admit_nodes(graph_id, [&node])
                    .map_err(EditRefusal::Placement)?;
                validate::seed_bindings(
                    graph,
                    scope,
                    Insertion {
                        node_id,
                        decl,
                        bindings: &bindings,
                    },
                )?;
                // Fixed here, not at write time, so a redo restores the depth the original add gave
                // instead of jumping in front of whatever arrived since.
                let placement = NodePlacement {
                    pos,
                    z: view.front_z(),
                };
                let mut node = node;
                node.name = node_name::free(graph, &node.name);
                let detached = DetachedNode::new(DetachedNodeParts {
                    node_id,
                    node,
                    bindings,
                    subscriptions: Vec::new(),
                    ghost_wiring: GhostWiring::default(),
                })
                .map_err(MalformedIntent::from)?;
                GraphStep::Node(Box::new(SetNode::insertion(detached, placement)))
            }
            Self::RemoveNode { node_id } => {
                validate::non_nil_node(node_id, Role::RemoveNode)?;
                // A subgraph keeps its `Outputs` node and one `Inputs` node; a selection that holds
                // them deletes the rest.
                let kept = match graph.find(node_id).and_then(Node::boundary) {
                    Some(Boundary::Outputs) => true,
                    Some(Boundary::Inputs) => {
                        graph
                            .iter()
                            .filter(|node| node.boundary() == Some(Boundary::Inputs))
                            .count()
                            == 1
                    }
                    None => false,
                };
                if kept {
                    return Err(EditRefusal::BoundaryNode.into());
                }
                let Some(entry) = NodeEntry::capture(document, graph_id, node_id) else {
                    return Ok(None);
                };
                let mut ties = TiesDraft::new(view);
                ties.release(entry.wires());
                ties.release(entry.ghost_wires());
                GraphStep::Node(Box::new(SetNode::removal(entry, ties.finish())))
            }
            Self::MoveSelection {
                members, offset, ..
            } => {
                let mut moves = Vec::with_capacity(members.len());
                for member in members.iter() {
                    let to = member.pos + offset;
                    validate::finite_pos(to, Role::MoveSelection)?;
                    // A member whose item vanished mid-gesture drops quietly.
                    let Some(from) = view.pos(member.item) else {
                        continue;
                    };
                    moves.push(Move {
                        item: member.item,
                        pos: Change { from, to },
                    });
                }
                GraphStep::Positions(SetPositions { moves })
            }
            Self::RenameNode { node_id, to } => {
                let Some(node) = validate::live_node(graph, node_id, Role::RenameNode)? else {
                    return Ok(None);
                };
                GraphStep::NodeName(SetNodeName {
                    node_id,
                    name: Change {
                        from: node.name.clone(),
                        to,
                    },
                })
            }
            Self::SetInput { input, to } => {
                let destination =
                    validate::live_node(graph, input.node_id, Role::SetInputDestination)?;
                let from = graph.input_state(input);
                let to = if let Some(binding) = to {
                    if destination.is_none() {
                        return Ok(None);
                    }
                    if let Binding::Wire(source) = &binding {
                        // A wire held across frames can outlive its producer.
                        if validate::live_node(graph, source.node_id, Role::SetInputProducer)?
                            .is_none()
                        {
                            return Ok(None);
                        }
                        // A compile refuses a cyclic graph. The canvas snap filter normally
                        // stops it earlier; this guards every binding path.
                        if graph.produces_cycle(source.node_id, input.node_id) {
                            return Ok(None);
                        }
                    }
                    graph.admit(scope, input, &binding).map_err(|rejection| {
                        MalformedIntent::Rejected {
                            port: input,
                            rejection,
                        }
                    })?;
                    InputState::Live(binding)
                } else {
                    // Clearing removes what the input holds wherever it sits, a ghost on a
                    // ghost node's input included. An input that loses a wire, live or ghost,
                    // holds the default its node declares for it, as `unwired::binding` gives.
                    let lost_wire = match &from {
                        InputState::Live(Binding::Wire(_)) => true,
                        InputState::Ghost(ghost) => ghost.output().is_some(),
                        InputState::Live(Binding::Const(_)) | InputState::Unbound => false,
                    };
                    let default = destination
                        .filter(|_| lost_wire)
                        .and_then(|node| node.decl(scope).ports().input(input.port))
                        .and_then(|declared| declared.default_value.clone());
                    InputState::from(default.map(Binding::Const))
                };
                if destination.is_none() && from == to {
                    return Ok(None);
                }
                let mut ties = TiesDraft::new(view);
                // A wire that leaves its output leaves its ties, a ghost wire as a live one; a
                // bind to the same output keeps them, as a restored ghost is the wire it was.
                let wired = match &from {
                    InputState::Live(Binding::Wire(output)) => Some(*output),
                    InputState::Ghost(ghost) => ghost.output(),
                    InputState::Live(Binding::Const(_)) | InputState::Unbound => None,
                };
                if let Some(output) = wired
                    && !matches!(to, InputState::Live(Binding::Wire(source)) if source == output)
                {
                    ties.release([WireKey::Data(DataWire { output, input })]);
                }
                GraphStep::Input(Box::new(SetInput {
                    input,
                    state: Change { from, to },
                    ties: ties.finish(),
                }))
            }
            Self::SetSelection { to } => {
                // The rubber band snapshots identities at drag start, so an interleaved undo can
                // remove one before release; keep only members the view still holds.
                let to: BTreeSet<ItemId> =
                    to.into_iter().filter(|&item| view.holds(item)).collect();
                GraphStep::Selection(SetSelection {
                    toggled: view.selected.symmetric_difference(&to).copied().collect(),
                })
            }
            Self::Raise { node_id } => {
                validate::non_nil_node(node_id, Role::Raise)?;
                let (Some(placement), Some(to)) =
                    (view.node_placements.get(&node_id), view.raised_z(node_id))
                else {
                    return Ok(None);
                };
                GraphStep::Depth(SetDepth {
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
                let property = match to {
                    NodeProperty::Disabled(to) => PropertyChange::Disabled(Change {
                        from: node.disabled,
                        to,
                    }),
                    NodeProperty::RuntimeCache(to) => PropertyChange::RuntimeCache(Change {
                        from: node
                            .cache()
                            .ok_or(MalformedIntent::NoCacheMode { node_id })?,
                        to,
                    }),
                    NodeProperty::Subgraph(to) => {
                        let refused = MalformedIntent::NotRepointable { node_id };
                        let from = node.subgraph().ok_or(refused)?;
                        let graphs = &document.graphs;
                        let (Some(held), Some(target)) =
                            (graphs.subgraph(from), graphs.subgraph(to))
                        else {
                            return Ok(None);
                        };
                        if held.interface != target.interface {
                            return Err(refused.into());
                        }
                        let mut placed = node.clone();
                        placed.set_subgraph(to);
                        graphs
                            .admit_nodes(graph_id, [&placed])
                            .map_err(EditRefusal::Placement)?;
                        PropertyChange::Subgraph(Change { from, to })
                    }
                };
                GraphStep::NodeProperty(SetNodeProperty { node_id, property })
            }
            Self::SetViewport { .. } => {
                unreachable!("`commit` writes the camera, which no step records")
            }
            Self::ReplaceNode { .. } => {
                unreachable!("`commit` lands a replace as two steps")
            }
            Self::SetSubscription {
                subscription,
                to: subscribe,
            } => {
                let Subscription { event, listener } = subscription;
                let subscriber = listener.node_id();
                validate::non_nil_node(event.node_id, Role::SetSubscriptionEmitter)?;
                validate::non_nil_node(subscriber, Role::SetSubscriptionSubscriber)?;
                // A subscribe needs both endpoints present. An unsubscribe of a vanished node
                // no-ops (from == to == false), so it needs no existence check.
                if subscribe {
                    if graph.find(event.node_id).is_none() || graph.find(subscriber).is_none() {
                        return Ok(None);
                    }
                    graph
                        .admit_subscription(scope, subscription)
                        .map_err(|rejection| MalformedIntent::RejectedSubscription {
                            subscription,
                            rejection,
                        })?;
                }
                let from = graph.subscription_state(subscription);
                let to = if subscribe {
                    SubscriptionState::Live
                } else {
                    SubscriptionState::Absent
                };
                let mut ties = TiesDraft::new(view);
                // An edge that goes leaves its ties, a ghost of one as a live one.
                if from != SubscriptionState::Absent && !subscribe {
                    ties.release([WireKey::Event(subscription)]);
                }
                GraphStep::Subscription(SetSubscription {
                    subscription,
                    state: Change { from, to },
                    ties: ties.finish(),
                })
            }
            Self::TieWires { tie, pos, wires } => {
                validate::non_nil_tie(tie, Role::TieWires)?;
                validate::finite_pos(pos, Role::TieWires)?;
                if view.ties.contains_key(&tie) {
                    return Err(MalformedIntent::TieAlreadyExists { tie }.into());
                }
                if wires.is_empty() {
                    return Err(MalformedIntent::EmptyTie { tie }.into());
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
                GraphStep::Ties(ties.finish())
            }
            // A tie gone since the gesture started is stale: no step.
            Self::JoinTie { tie, wires } => {
                validate::non_nil_tie(tie, Role::JoinTie)?;
                if !view.ties.contains_key(&tie) {
                    return Ok(None);
                }
                let mut ties = TiesDraft::new(view);
                insert_routes(graph, &mut ties, tie, &wires)?;
                GraphStep::Ties(ties.finish())
            }
            Self::RemoveTie { tie } => {
                validate::non_nil_tie(tie, Role::RemoveTie)?;
                if !view.ties.contains_key(&tie) {
                    return Ok(None);
                }
                let mut ties = TiesDraft::new(view);
                ties.remove_tie(tie);
                GraphStep::Ties(ties.finish())
            }
            Self::SetTieAngle { tie, angle, .. } => {
                validate::non_nil_tie(tie, Role::SetTieAngle)?;
                if let TieAngle::Pinned(degrees) = angle
                    && !angle.is_valid()
                {
                    return Err(MalformedIntent::InvalidTieAngle { tie, degrees }.into());
                }
                let Some(&placed) = view.ties.get(&tie) else {
                    return Ok(None);
                };
                let mut ties = TiesDraft::new(view);
                ties.set_tie(tie, Some(Tie { angle, ..placed }));
                GraphStep::Ties(ties.finish())
            }
            // A menu stays open across frames, so the wire may have left the
            // tie since it opened.
            Self::ReleaseWire { tie, wire } => {
                validate::non_nil_tie(tie, Role::ReleaseWire)?;
                if !view
                    .routes
                    .get(&wire)
                    .is_some_and(|route| route.contains(tie))
                {
                    return Ok(None);
                }
                let mut ties = TiesDraft::new(view);
                ties.leave(wire, tie);
                GraphStep::Ties(ties.finish())
            }
        };
        Ok(Some(step))
    }

    /// Whether the intent writes the graph itself, not the view of it: what a read-only
    /// subgraph refuses.
    const fn writes_graph(&self) -> bool {
        match self {
            Self::AddNode { .. }
            | Self::RemoveNode { .. }
            | Self::ReplaceNode { .. }
            | Self::RenameNode { .. }
            | Self::SetInput { .. }
            | Self::SetNodeProperty { .. }
            | Self::SetSubscription { .. } => true,
            Self::MoveSelection { .. }
            | Self::SetSelection { .. }
            | Self::Raise { .. }
            | Self::SetViewport { .. }
            | Self::TieWires { .. }
            | Self::JoinTie { .. }
            | Self::RemoveTie { .. }
            | Self::SetTieAngle { .. }
            | Self::ReleaseWire { .. } => false,
        }
    }

    /// The held gesture this intent is a frame of, if any.
    pub(crate) const fn gesture(&self) -> Option<GestureId> {
        match self {
            Self::MoveSelection { gesture, .. } => Some(*gesture),
            Self::SetTieAngle { gesture, .. } => *gesture,
            _ => None,
        }
    }

    /// Build, no-op-filter, and apply one intent against `graph` of `document`: the entry every
    /// frontend drives its per-intent loop through.
    ///
    /// Pushes each committed [`UndoStep`] onto `steps`, in the order applied: none when nothing
    /// came of the intent (a stale anchor, a cycle-forming bind, or a no-op). A node's removal is
    /// followed by a [`Self::SetInput`] for each input it fed that takes its default back, as a
    /// wire removed on its own leaves it ([`unwired::binding`]). Only a payload that could never
    /// have applied is an `Err` (see [`Self::into_step`]).
    ///
    /// [`UndoStep::apply`] stays separate for redo, which replays already-valid history.
    pub(crate) fn commit(
        self,
        document: &mut Document,
        graph: GraphId,
        library: &Library,
        steps: &mut Vec<UndoStep>,
    ) -> Result<(), IntentError> {
        if self.writes_graph()
            && let Some(why) = document.read_only(graph)
        {
            return Err(EditRefusal::ReadOnly { why }.into());
        }
        if let Self::SetViewport { to } = self {
            if !to.is_valid() {
                return Err(MalformedIntent::InvalidViewport.into());
            }
            document.slot_mut(graph).view.viewport = to;
            return Ok(());
        }
        if let Self::ReplaceNode {
            node_id,
            with,
            remap,
        } = self
        {
            return replace(
                document,
                graph,
                library,
                ReplaceNode {
                    node_id,
                    with,
                    remap,
                },
                steps,
            );
        }
        let Some(step) = self.into_step(document, graph, library)? else {
            return Ok(());
        };
        // Read off the removal's own record, which captured every wire it ends.
        let fed: Vec<InputPort> = match &step {
            GraphStep::Node(node) => node.fed_inputs().collect(),
            _ => Vec::new(),
        };
        let step = UndoStep::Graph { graph, step };
        if step.is_noop() {
            return Ok(());
        }
        step.apply(document);
        steps.push(step);
        for input in fed {
            let graphs = &document.graphs;
            if let Some(to) =
                unwired::binding(graphs.graph(graph), graphs.scope(library, graph), input)
            {
                Self::SetInput {
                    input,
                    to: Some(to),
                }
                .commit(document, graph, library, steps)?;
            }
        }
        Ok(())
    }
}

/// The parts of a [`GraphIntent::ReplaceNode`].
#[derive(Debug)]
struct ReplaceNode {
    node_id: NodeId,
    with: Node,
    remap: NodeRemap,
}

/// Commit a replace: the node's removal, with the routes of its wires, then the record
/// [`CoreGraph::replaced`] makes of it put in where it was, as two steps of one entry. A node the
/// graph no longer holds is a stale intent; a `with` that does not resolve, a boundary node, or a
/// placement the set refuses is refused.
fn replace(
    document: &mut Document,
    graph_id: GraphId,
    library: &Library,
    replace: ReplaceNode,
    steps: &mut Vec<UndoStep>,
) -> Result<(), IntentError> {
    let ReplaceNode {
        node_id,
        with,
        remap,
    } = replace;
    validate::non_nil_node(node_id, Role::RemoveNode)?;
    let Some(entry) = NodeEntry::capture(document, graph_id, node_id) else {
        return Ok(());
    };
    let (graph, view) = (document.graphs.graph(graph_id), document.view(graph_id));
    let scope = document.graphs.scope(library, graph_id);
    validate::resolving_node(scope, &with)?;
    if with.boundary().is_some()
        || graph
            .find(node_id)
            .is_some_and(|node| node.boundary().is_some())
    {
        return Err(EditRefusal::BoundaryNode.into());
    }
    document
        .graphs
        .admit_nodes(graph_id, [&with])
        .map_err(EditRefusal::Placement)?;
    let record = graph.replaced(scope, node_id, with, &remap);
    let mut ties = TiesDraft::new(view);
    ties.release(entry.wires());
    ties.release(entry.ghost_wires());
    let insertion = SetNode::insertion_in_place(&entry, record);
    for step in [
        GraphStep::Node(Box::new(SetNode::removal(entry, ties.finish()))),
        GraphStep::Node(Box::new(insertion)),
    ] {
        let step = UndoStep::Graph {
            graph: graph_id,
            step,
        };
        step.apply(document);
        steps.push(step);
    }
    Ok(())
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
            .map_or_default(|route| route.iter().copied().collect());
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
    use scenarium::{GraphId, Library, Node, NodeId};

    use crate::core::document::Document;
    use crate::core::edit::error::{IntentError, MalformedIntent};
    use crate::core::edit::graph_intent::GraphIntent;
    use crate::core::edit::step::undo_step::UndoStep;

    impl GraphIntent {
        /// [`Self::commit`] on the main graph of an intent that commits at most one step.
        pub(crate) fn commit_one(
            self,
            document: &mut Document,
            library: &Library,
        ) -> Result<Option<UndoStep>, MalformedIntent> {
            let mut steps = Vec::new();
            match self.commit(document, GraphId::Main, library, &mut steps) {
                Ok(()) | Err(IntentError::Refused(_)) => {}
                Err(IntentError::Malformed(malformed)) => return Err(malformed),
            }
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
