//! What a widget wants the graph to look like, and how that lands.
//!
//! A [`GraphIntent`] is forward-only — "set X to Y", no history — and is the
//! vocabulary every surface of the editor speaks. Turning one into something
//! reversible is [`GraphIntent::into_step`]: it reads the state the intent
//! overwrites out of a `&Document` and folds the two into an
//! [`UndoStep`](crate::core::edit::step::undo_step::UndoStep).
//!
//! The two vocabularies are deliberately *not* parallel. Intents are named
//! for what a user did, steps for what the document can take back, and the
//! lowering between them is free to expand one into several (duplicating a
//! selection) or to send two opposite intents to one step (adding and
//! removing a node). Adding an intent is a variant plus an arm here; adding a
//! step is a file in [`step`](crate::core::edit::step).
//!
//! The same split runs the other way, by *scope*: a `GraphIntent` edits the
//! graph, while a [`DockOp`](crate::core::document::dock::DockOp) edits the
//! layout around it. Neither can be mistaken for the other, so no code path
//! has to carry state it will not read.

use std::collections::{BTreeSet, HashMap};

use glam::Vec2;
use scenarium::{Binding, DetachedNode, InputPort, Node, NodeId, Subscription};

use crate::core::document::PortRef;
use crate::core::document::{Document, ItemPlacement, Viewport};
use crate::core::edit::error::MalformedIntent;
use crate::core::edit::step::change::Change;
use crate::core::edit::step::move_selection::{Move, MoveSelection};
use crate::core::edit::step::node_presence::{NodePresence, NodeState};
use crate::core::edit::step::raise::Raise;
use crate::core::edit::step::rename_node::RenameNode;
use crate::core::edit::step::set_input::SetInput;
use crate::core::edit::step::set_node_property::{NodeProperty, SetNodeProperty};
use crate::core::edit::step::set_selection::SetSelection;
use crate::core::edit::step::set_subscription::SetSubscription;
use crate::core::edit::step::set_viewport::SetViewport;
use crate::core::edit::step::undo_step::UndoStep;
use crate::core::edit::validate;

/// World-space offset applied to duplicated nodes so the copies don't land
/// exactly on top of their originals.
const DUPLICATE_OFFSET: Vec2 = Vec2::new(32.0, 32.0);

/// What the caller wants to change in the document's graph. Forward-only — no
/// `from` fields: each variant says "set X to Y", and the previous Y is
/// captured at commit time by [`Self::into_step`].
///
/// Every variant here edits the graph, and travels the frame's queue as its
/// `Graph` tier. A mutation of the layout instead of the graph is a
/// [`DockOp`](crate::core::document::dock::DockOp), which travels the same
/// queue in the `View` tier.
#[derive(Debug)]
pub(crate) enum GraphIntent {
    /// Add one node that links state the document already resolves: a func in
    /// the library, or a built-in special.
    AddNode {
        /// Where the node lands on the canvas; its view item is created
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
        bindings: Vec<(InputPort, Binding)>,
    },
    RemoveNode {
        node_id: NodeId,
    },
    /// Drag-move one or more selected node bodies in canvas-world
    /// coordinates. A multi-select drag moves the whole group as a single
    /// undo entry; a plain drag carries just the one grabbed item. `grabbed`
    /// is whichever member the pointer latched — it keys the drag gesture so
    /// consecutive frames coalesce.
    MoveSelection {
        grabbed: NodeId,
        /// `(item, target position)` per moved member.
        moves: Vec<(NodeId, Vec2)>,
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
        to: BTreeSet<NodeId>,
    },
    /// Lift an item — a node body or a pinned output's preview widget — to
    /// the top of its graph's paint stack, by setting its
    /// [`ItemPlacement::z`] past every other. Emitted when either kind is
    /// clicked or grabbed, so clicking brings it forward. The depth is stored
    /// view state, so it persists across save/load and tab switches and walks
    /// with undo/redo — unlike the transient selection-recency stack it
    /// replaced.
    Raise {
        key: NodeId,
    },
    /// Set one scalar property of a node — its `disabled` flag or its cache
    /// mode. See [`NodeProperty`].
    SetNodeProperty {
        node_id: NodeId,
        to: NodeProperty,
    },
    SetViewport {
        to: Viewport,
    },
    /// Add (`subscribe = true`) or remove (`false`) an event subscription: an
    /// event wire dropped on, or severed from, a subscription pin.
    /// Idempotent — a no-op when the subscription already matches.
    SetSubscription {
        subscription: Subscription,
        subscribe: bool,
    },
}

impl GraphIntent {
    /// The intents a click on `key` produces: the selection change, plus a
    /// lift to the top of the paint stack so a clicked node comes to the
    /// front. The raise is skipped only when a Shift-click *removes* `key`
    /// from the selection — a node you just deselected shouldn't jump
    /// forward. Shared by the node body, its header title and its port
    /// labels, so clicking any of them behaves like clicking the body.
    ///
    /// Takes the committed selection rather than a UI context: that is the
    /// only thing either rule reads, and it keeps the click policy beside the
    /// intents it builds instead of in the widget that raises them.
    pub(crate) fn click(
        shift: bool,
        selected: &BTreeSet<NodeId>,
        key: NodeId,
    ) -> impl Iterator<Item = Self> {
        let deselecting = shift && selected.contains(&key);
        std::iter::once(Self::select_click(shift, selected, key))
            .chain((!deselecting).then_some(Self::Raise { key }))
    }

    /// The [`Self::SetSelection`] a click on `key` produces: a plain click
    /// selects only it, a Shift-click toggles its membership. A click that
    /// changes nothing is dropped at commit as a no-op.
    fn select_click(shift: bool, selected: &BTreeSet<NodeId>, key: NodeId) -> Self {
        let mut to = if shift {
            selected.clone()
        } else {
            BTreeSet::new()
        };
        if shift && selected.contains(&key) {
            to.remove(&key);
        } else {
            to.insert(key);
        }
        Self::SetSelection { to }
    }

    /// Drop the whole selection. Named rather than spelled `SetSelection`
    /// with an empty set at each call, so a reader sees the intent and not
    /// the mechanism.
    pub(crate) fn clear_selection() -> Self {
        Self::SetSelection {
            to: BTreeSet::new(),
        }
    }

    /// [`Self::SetInput`] over a [`PortRef`] — the UI's port coordinate — so a
    /// widget that has one does not restate the `PortRef` → [`InputPort`]
    /// conversion at every call. `None` clears the binding.
    pub(crate) fn set_input(port: PortRef, to: impl Into<Option<Binding>>) -> Self {
        Self::SetInput {
            input: InputPort::new(port.node_id, port.port_idx),
            to: to.into(),
        }
    }

    /// Clone `doc`'s current selection: each node gets a fresh id and an
    /// offset position, const-value bindings copy verbatim, the data and
    /// event connections *among* the selected nodes are recreated against the
    /// clones, and the copies end up selected. A `Bind` whose source is
    /// *outside* the selection is dropped unless `include_incoming` is set, in
    /// which case the clone keeps the wire pointing at the original external
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
    /// The selection is the only source of a duplicate set: Ctrl+D and the
    /// node context menu's two Duplicate picks all act on it, the latter
    /// because a right-click selects the node it landed on first.
    pub(crate) fn duplicate(doc: &Document, include_incoming: bool) -> Vec<Self> {
        let (graph, view) = (&doc.graph, &doc.main_view);
        // Resolve the whole set first, in selection order: an edge is only
        // recognisable as internal once both of its endpoints have a fresh id,
        // so nothing can be authored until every member has one. A selected
        // node the graph no longer holds simply isn't part of the set.
        let mut clones: HashMap<NodeId, NodeId> = HashMap::with_capacity(view.selected.len());
        let mut sources = Vec::with_capacity(view.selected.len());
        for old_id in &view.selected {
            let Some(node) = graph.find(*old_id) else {
                continue;
            };
            let new_id = NodeId::unique();
            clones.insert(*old_id, new_id);
            sources.push((*old_id, new_id, node));
        }
        let mut intents = Vec::new();
        if sources.is_empty() {
            return intents;
        }

        let mut internal = Vec::new();
        for (old_id, new_id, node) in sources {
            let pos = view
                .item_placements
                .get(&old_id)
                .expect("the view places every node the graph holds")
                .pos
                + DUPLICATE_OFFSET;
            // This node's *own* inputs. `bindings_touching` would also hand
            // back every binding that *reads* the node — cloned into a fresh
            // `Vec`, then discarded. `InputPort` orders by `(node_id,
            // port_idx)`, so a node's inputs sit contiguously.
            let own_inputs = graph
                .bindings
                .range(InputPort::new(old_id, 0)..)
                .take_while(|(port, _)| port.node_id == old_id);
            let mut bindings = Vec::new();
            for (port, binding) in own_inputs {
                let input = InputPort::new(new_id, port.port_idx);
                match binding {
                    Binding::Bind(source) => match clones.get(&source.node_id) {
                        Some(&new_source) => internal.push(Self::SetInput {
                            input,
                            to: Some(Binding::bind(new_source, source.port_idx)),
                        }),
                        None if include_incoming => bindings.push((input, Binding::Bind(*source))),
                        None => {}
                    },
                    other => bindings.push((input, other.clone())),
                }
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
                clones.get(&subscription.emitter),
                clones.get(&subscription.subscriber),
            ) {
                intents.push(Self::SetSubscription {
                    subscription: Subscription {
                        emitter,
                        event_idx: subscription.event_idx,
                        subscriber,
                    },
                    subscribe: true,
                });
            }
        }
        intents.push(Self::SetSelection {
            to: clones.values().copied().collect(),
        });
        intents
    }

    /// Read the state this intent overwrites out of `doc` and fold the two
    /// into a complete [`UndoStep`]. Pure — does not write to the graph.
    ///
    /// This is the only gate between a caller and the document, so each arm
    /// establishes the full precondition set the step's `write` half assumes:
    /// an `Ok` result is a proof that replaying the step trips no assert on
    /// the way and leaves the document passing [`Document::validate`].
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
    pub(crate) fn into_step(self, doc: &Document) -> Result<Option<UndoStep>, MalformedIntent> {
        let (graph, view) = (&doc.graph, &doc.main_view);
        let step = match self {
            Self::AddNode {
                pos,
                node_id,
                node,
                bindings,
            } => {
                validate::fresh_node_id(graph, node_id)?;
                validate::finite_position(pos, "AddNode")?;
                validate::insertable_kind(&node)?;
                let bindings = validate::seed_bindings(graph, node_id, bindings)?;
                // The depth is fixed here rather than at write time, so a redo
                // puts the node back at the depth the original add gave it
                // instead of jumping it in front of whatever arrived since.
                let placement = ItemPlacement {
                    pos,
                    z: view.front_z(),
                };
                UndoStep::NodePresence(NodePresence::insertion(
                    DetachedNode {
                        node_id,
                        node,
                        bindings,
                        subscriptions: Vec::new(),
                    },
                    placement,
                ))
            }
            Self::RemoveNode { node_id } => {
                validate::non_nil_node(node_id, "RemoveNode")?;
                let Some(state) = NodeState::capture(doc, node_id) else {
                    return Ok(None);
                };
                UndoStep::NodePresence(NodePresence::removal(state))
            }
            Self::MoveSelection { grabbed, moves } => {
                let mut placed = Vec::with_capacity(moves.len());
                for (key, to) in moves {
                    validate::finite_position(to, "MoveSelection")?;
                    // Drag-sourced (spans frames): a member whose item
                    // vanished mid-gesture (node removed) drops quietly.
                    let Some(placement) = view.item_placements.get(&key) else {
                        continue;
                    };
                    placed.push(Move {
                        key,
                        pos: Change {
                            from: placement.pos,
                            to,
                        },
                    });
                }
                UndoStep::MoveSelection(MoveSelection {
                    grabbed,
                    moves: placed,
                })
            }
            Self::RenameNode { node_id, to } => {
                let Some(node) = validate::live_node(graph, node_id, "RenameNode")? else {
                    return Ok(None);
                };
                UndoStep::RenameNode(RenameNode {
                    node_id,
                    name: Change {
                        from: node.name.clone(),
                        to,
                    },
                })
            }
            Self::SetInput { input, to } => {
                if validate::live_node(graph, input.node_id, "SetInput destination")?.is_none() {
                    return Ok(None);
                }
                if let Some(Binding::Bind(source)) = &to {
                    // A wire held across frames can outlive its producer, and
                    // the bind would leave the graph with a dangling edge.
                    if validate::live_node(graph, source.node_id, "SetInput producer")?.is_none() {
                        return Ok(None);
                    }
                    // Reject a bind that would close a data cycle: the planner
                    // rejects a cyclic graph outright (`Error::CycleDetected`),
                    // so the edit must never land. The GUI snap filter normally
                    // stops this earlier; this is the authoritative guard
                    // covering every binding path, including any that bypass
                    // the canvas.
                    if graph.produces_cycle(source.node_id, input.node_id) {
                        return Ok(None);
                    }
                }
                UndoStep::SetInput(SetInput {
                    input,
                    binding: Change {
                        from: graph.bindings.get(&input).cloned(),
                        to,
                    },
                })
            }
            Self::SetSelection { to } => UndoStep::SetSelection(SetSelection {
                selection: Change {
                    from: view.selected.clone(),
                    // The rubber band snapshots identities when the drag
                    // starts, so an interleaved undo can remove one before
                    // release. Keep the members that still have a widget
                    // rather than recording a selection the view can't render.
                    to: to
                        .into_iter()
                        .filter(|key| view.item_placements.contains_key(key))
                        .collect(),
                },
            }),
            Self::Raise { key } => {
                let Some(placement) = view.item_placements.get(&key) else {
                    return Ok(None);
                };
                UndoStep::Raise(Raise {
                    key,
                    z: Change {
                        from: placement.z,
                        to: view.front_z(),
                    },
                })
            }
            Self::SetNodeProperty { node_id, to } => {
                let Some(node) = validate::live_node(graph, node_id, "SetNodeProperty")? else {
                    return Ok(None);
                };
                // Capture the *same* property's current value as `from`, so a
                // revert writes a disable flag over a disable flag.
                let from = match to {
                    NodeProperty::Disabled(_) => NodeProperty::Disabled(node.disabled),
                    NodeProperty::RuntimeCache(_) => NodeProperty::RuntimeCache(node.cache),
                };
                UndoStep::SetNodeProperty(SetNodeProperty {
                    node_id,
                    property: Change { from, to },
                })
            }
            Self::SetViewport { to } => {
                if !to.is_valid() {
                    return Err(MalformedIntent::InvalidViewport);
                }
                UndoStep::SetViewport(SetViewport {
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
                let Subscription {
                    emitter,
                    event_idx,
                    subscriber,
                } = subscription;
                validate::non_nil_node(emitter, "SetSubscription emitter")?;
                validate::non_nil_node(subscriber, "SetSubscription subscriber")?;
                // A subscribe needs both endpoints present; a stale drag onto a
                // vanished node drops rather than recording a dangling
                // subscription. An unsubscribe of a vanished node no-ops
                // naturally (nothing is subscribed → from == to == false), so
                // it needs no existence check.
                if subscribe && (graph.find(emitter).is_none() || graph.find(subscriber).is_none())
                {
                    return Ok(None);
                }
                UndoStep::SetSubscription(SetSubscription {
                    subscription,
                    subscribed: Change {
                        from: graph.is_subscribed(emitter, event_idx, subscriber),
                        to: subscribe,
                    },
                })
            }
        };
        Ok(Some(step))
    }

    /// Build, no-op-filter, and apply one intent against `doc` in a single
    /// call — the entry every frontend drives its per-intent loop through. A
    /// `SetInput` that retypes wildcard outputs severs nothing: type
    /// mismatches are tolerated (the wire draws as mismatched and lowers as
    /// unbound — see scenarium's `typed_binding`), so the edit stays a single
    /// step.
    ///
    /// Returns the committed [`UndoStep`] (the caller records it and reads its
    /// signals), or `Ok(None)` when nothing came of the intent and nothing was
    /// written — a stale anchor, a cycle-forming bind, or a no-op. Only a
    /// payload that could never have applied is an `Err` (see
    /// [`Self::into_step`]).
    ///
    /// [`Self::into_step`] and [`UndoStep::apply`] stay separate for the
    /// undo-stack redo path, which applies a stored step without rebuilding it
    /// (a redo replays already-valid history).
    pub(crate) fn commit(self, doc: &mut Document) -> Result<Option<UndoStep>, MalformedIntent> {
        let Some(step) = self.into_step(doc)? else {
            return Ok(None);
        };
        if step.is_noop() {
            return Ok(None);
        }
        step.apply(doc);
        Ok(Some(step))
    }
}

#[cfg(test)]
mod tests;
