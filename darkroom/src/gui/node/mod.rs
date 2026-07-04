pub(crate) mod header;
pub(crate) mod port_color;
pub(crate) mod port_rename;
pub(crate) mod port_row;
pub(crate) mod value_editor;

use crate::core::document::GraphRef;
use crate::core::edit::intent::Intent;
use crate::gui::canvas::breaker::BreakerProbe;
use crate::gui::canvas::inspector::Inspectors;
use crate::gui::canvas::node_ports;
use crate::gui::canvas::port_frame::PortFrame;
use crate::gui::node::header::{header, status_row, subgraph_badge_wid};
use crate::gui::node::port_row::{const_editor_wid, input_cell_wid, port_circle_wid, ports_row};
use crate::gui::run_state::ExecStatus;
use crate::gui::scene::{InputBindingView, Scene, SceneNode};
use crate::gui::theme::Theme;
use crate::gui::{PortKind, PortRef, UiAction};
use glam::Vec2;
use palantir::{
    Background, Color, Configure, Corners, Panel, Rect, Sense, Shadow, Sizing, Stroke, Ui, WidgetId,
};
use scenarium::data::{DataType, FsPathConfig, StaticValue};
use scenarium::graph::Binding;
use scenarium::graph::NodeId;
use scenarium::graph::subgraph::SubgraphRef;
use scenarium::library::Library;
use std::collections::BTreeSet;
use std::sync::Arc;

/// Read-only context the node-draw chain threads top to bottom: the
/// theme, the scene being rendered, and last frame's port geometry.
/// `Copy` (all shared refs), so it's passed by value — copying it while
/// a `&rcx.scene.nodes` borrow is live is fine, which keeps
/// `draw_all`'s node loop borrow-clean. The mutable sinks (`out`,
/// `actions`) and the breaker `probe` stay separate params.
#[derive(Clone, Copy)]
pub(crate) struct RecordCtx<'a> {
    pub(crate) theme: &'a Theme,
    /// The runtime library, for resolving a port's registered type metadata
    /// (display name, enum variants) — `DataType` carries only the id.
    pub(crate) library: &'a Library,
    pub(crate) scene: &'a Scene,
    /// Effective selection to paint: the committed set
    /// (`scene.selected_nodes`) or, mid-rubber-band, the live swept
    /// preview owned by `SelectionUI`. Kept off `Scene` so the projection
    /// stays a read-only mirror — the gesture no longer scribbles its
    /// preview into the committed field.
    pub(crate) selected: &'a BTreeSet<NodeId>,
    pub(crate) port_frame: &'a PortFrame,
    /// Open inspection panels, so the header chip can render its
    /// open/pinned state.
    pub(crate) inspectors: &'a Inspectors,
}

/// Owns rendering of every graph node plus the single active drag
/// anchor — the press-frame positions are snapshotted here so each
/// `MoveNodes` target is `start_pos + drag_delta`, not a running
/// integration over the moving source. Only one node can hold the
/// pointer at a time, so one anchor slot is enough.
///
/// `draw_all` is the single entry point; `GraphUI` calls it once per
/// frame after [`crate::gui::canvas::port_frame::PortFrame`] has been rebuilt
/// from last-frame's responses.
#[derive(Default, Debug)]
pub(crate) struct NodeUI {
    drag_anchor: Option<DragAnchor>,
    /// Back-to-front node paint order (most-recently-selected on top). Reset
    /// on tab switch with the rest of the gesture state, so it only ever
    /// holds the active graph's nodes.
    z_order: ZOrder,
}

#[derive(Clone, Debug)]
struct DragAnchor {
    /// The node the pointer latched. Keys the `response_for` lookup and
    /// the drag gesture; always present in `start_positions`.
    node_id: NodeId,
    /// Every node moving with this drag and its position at drag start:
    /// the whole selection when the grabbed node was already selected,
    /// else just the grabbed node. Each emits `start + delta`.
    start_positions: Vec<(NodeId, Vec2)>,
    /// Captured from the `drag_started` frame's `Response::widget_id()`
    /// so subsequent frames can `ui.response_for(widget_id)` *before*
    /// recording and bake the current `drag_delta` into `.position(...)`.
    /// Lets the node paint at the cursor's location in Pass A directly
    /// — no need to wait for Pass B's relayout to catch up.
    widget_id: WidgetId,
}

impl NodeUI {
    /// Iterate every scene node, recording its widget tree and
    /// pushing port circle centers into `centers`. Inserts into
    /// `port_nodes` only when every port resolved a layout rect.
    /// Emits an `Intent::MoveNodes` for any node holding an active
    /// LMB drag on its body (port circles capture their own clicks
    /// via `Sense::CLICK` so drags don't latch off the port grabs).
    pub(crate) fn draw_all(
        &mut self,
        ui: &mut Ui,
        rcx: RecordCtx<'_>,
        probe: &mut BreakerProbe<'_>,
        out: &mut Vec<Intent>,
    ) {
        if let Some(b) = probe.state.as_deref_mut() {
            b.broken_nodes.clear();
        }
        // Paint back-to-front in selection-recency order — later draws sit
        // on top, so the most recently selected node is frontmost.
        let order = self.paint_order(rcx.scene);
        for n in order {
            self.draw_one(ui, rcx, n, probe, out);
        }
        // Drop the anchor if its target node vanished from the graph
        // (mid-drag delete). Without this, the slot would linger and
        // could fire when a fresh node reused the id.
        if let Some(a) = &self.drag_anchor
            && rcx.scene.nodes.by_key(&a.node_id).is_none()
        {
            self.drag_anchor = None;
        }
    }

    /// The scene's nodes in back-to-front paint order. Folds this frame's
    /// committed selection into the recency [`Self::z_order`] (newly-selected
    /// nodes rise to the top) and resolves it to `&SceneNode`s. Uses
    /// `scene.selected_nodes` (committed), not the rubber-band preview, so
    /// recency updates only when a selection commits — not every frame of a
    /// band drag.
    fn paint_order<'a>(&mut self, scene: &'a Scene) -> Vec<&'a SceneNode> {
        let current: Vec<NodeId> = scene.nodes.iter().map(|n| n.id).collect();
        let order = self.z_order.reconcile(&current, &scene.selected_nodes);
        order
            .iter()
            .filter_map(|id| scene.nodes.by_key(id))
            .collect()
    }

    fn draw_one(
        &mut self,
        ui: &mut Ui,
        rcx: RecordCtx<'_>,
        node: &SceneNode,
        probe: &mut BreakerProbe<'_>,
        out: &mut Vec<Intent>,
    ) {
        let theme = rcx.theme;

        // Probe last-frame's body rect (in canvas world coords) against
        // the breaker polyline. Hit → recolor border red and flag the
        // node for deletion on release. First-frame nodes have no rect
        // yet, so the breaker simply can't catch them until next frame
        // — acceptable: the user can't aim at something that hasn't
        // been painted.
        let body_rect = ui
            .response_for(node_widget_id(node.id))
            .layout_rect
            .map(|r| Rect {
                min: r.min - probe.origin,
                size: r.size,
            });
        let broken = match (probe.state.as_deref(), body_rect) {
            (Some(b), Some(r)) => b.intersects_rect(r),
            _ => false,
        };
        if broken {
            // unwrap: `broken == true` implies `state` is `Some`.
            probe
                .state
                .as_deref_mut()
                .unwrap()
                .broken_nodes
                .push(node.id);
        }
        let selected = rcx.selected.contains(&node.id);
        // The border width is *always* the selection width so selecting a
        // node never resizes it (stroke folds into padding). Only the
        // color changes: the breaker alarm wins, then the bright selection
        // halo, else the resting `node_border` (a faint outline just above
        // the fill).
        let border_width = theme.node_border_width * 2.0;
        let border = if broken {
            theme.connection_broken
        } else if node.missing {
            // A stub for a node whose func is gone from the library: paint it
            // in the error color so it reads as broken-but-deletable.
            theme.exec_errored_glow
        } else if selected {
            theme.text_muted
        } else {
            theme.node_border
        };
        // Sample modifiers before the panel borrows `ui` for the rest
        // of this scope (the click handler below can't reborrow it).
        let shift_click = ui.modifiers().shift;
        // Soft glow behind the node colored by its last run outcome
        // (zero-offset so it wraps evenly). `None` paints nothing.
        let shadow = exec_shadow(theme, node.exec_status);

        let panel = Panel::vstack()
            .id(node_widget_id(node.id))
            .position(node.pos)
            .min_size((theme.node_min_width, theme.node_min_height))
            .size((Sizing::Hug, Sizing::Hug))
            .sense(Sense::CLICK | Sense::DRAG)
            .background(Background {
                fill: theme.node_fill.into(),
                stroke: Stroke::solid(border, border_width),
                corners: Corners::all(theme.node_corner_radius),
                shadow,
            })
            .show(ui, |ui| {
                header(ui, rcx, node, out);
                status_row(ui, rcx, node, out);
                ports_row(ui, rcx, node, out);
            });
        // Pull the body response's flags into locals so its `&mut ui`
        // borrow ends before the `response_for(title)` peek below.
        let response = panel.response;
        let body_clicked = response.clicked();
        let body_drag_started = response.drag_started();
        let body_wid = response.widget_id();

        // Click without drag → select. Plain click selects only this
        // node; Shift-click toggles its membership in the current
        // selection. `UndoStep::is_noop` filters a click that doesn't
        // change the set (e.g. clicking the sole selected node).
        if body_clicked {
            out.push(select_intent(shift_click, rcx.scene, node.id));
        }

        // The header title doubles as a drag handle: its idle label
        // senses `DRAG`, so a drag latched there moves the node like a
        // body drag (the title swallows the press, so the body never sees
        // it). `response_for` is last-frame, matching how `prepass` reads
        // the delta. While renaming the title is a `TextEdit` (no `DRAG`),
        // so this can't fire mid-edit.
        let title_wid = node_rename_wid(node.id);
        let title_drag = ui.response_for(title_wid).drag_started();

        // Latch the anchor on the press-frame edge; subsequent frames'
        // `prepass` peeks `response_for(widget_id)` before record runs
        // and converts `drag_delta` into a `MoveNodes` applied to
        // `Document` upstream of `Scene::rebuild`.
        if title_drag || body_drag_started {
            // Grabbing a node already in the selection drags the whole
            // group together; grabbing an unselected node selects only it
            // and drags it alone.
            let start_positions = if selected {
                rcx.scene
                    .nodes
                    .iter()
                    .filter(|n| rcx.selected.contains(&n.id))
                    .map(|n| (n.id, n.pos))
                    .collect()
            } else {
                out.push(select_intent(false, rcx.scene, node.id));
                vec![(node.id, node.pos)]
            };
            self.drag_anchor = Some(DragAnchor {
                node_id: node.id,
                start_positions,
                widget_id: if title_drag { title_wid } else { body_wid },
            });
        }
    }

    /// Pre-record pass: peek palantir's input state for any widgets
    /// this `NodeUI` owns and push the corresponding `Intent`s into
    /// `out`. Runs before `Scene::rebuild` in `App::frame`, so any
    /// state mutation applied from these intents (notably drag-driven
    /// `MoveNodes`) lands in `Document` before recording — Pass A's
    /// arrange already reflects the cursor; no Pass B relayout retry.
    pub(crate) fn prepass(&mut self, ui: &Ui, scene: &Scene, out: &mut Vec<Intent>) {
        // `node_id`/`widget_id` are `Copy`, so pull them out and drop the
        // borrow — that lets the early returns below reassign
        // `self.drag_anchor` without cloning the `start_positions` `Vec`,
        // which is only read in the success path (where the anchor isn't
        // cleared and can be re-borrowed).
        let Some(&DragAnchor {
            node_id, widget_id, ..
        }) = self.drag_anchor.as_ref()
        else {
            return;
        };
        // Drop a stale anchor whose node was removed last frame (e.g.
        // breaker swipe deleted the dragged node). Without this, the
        // emitted `MoveNodes` would target a missing node and panic in
        // `build_step`. `draw_all` also clears stale anchors, but only
        // after this prepass runs.
        if scene.nodes.by_key(&node_id).is_none() {
            self.drag_anchor = None;
            return;
        }
        let resp = ui.response_for(widget_id);
        // `drag_started` on a still-active anchor means a *new* gesture
        // just latched on the same widget — `record` will replace the
        // anchor this frame; emitting now with the stale start positions
        // makes the nodes snap to the previous gesture's start point.
        if resp.drag_started() {
            self.drag_anchor = None;
            return;
        }
        // No `drag_delta` means the drag isn't latched anymore (release
        // or pointer-left-surface). Drop the anchor so the next gesture
        // starts fresh.
        let Some(delta) = resp.drag_delta() else {
            self.drag_anchor = None;
            return;
        };
        // `drag_delta` is in screen pixels; node positions live in the
        // canvas's pre-transform frame. Divide by zoom so cursor travel
        // matches node travel at every zoom level.
        let zoom = if scene.viewport.zoom > 0.0 {
            scene.viewport.zoom
        } else {
            1.0
        };
        let offset = delta / zoom;
        // Anchor still present (success path never cleared it); re-borrow
        // to read the start positions without cloning.
        let to = self
            .drag_anchor
            .as_ref()
            .unwrap()
            .start_positions
            .iter()
            .map(|(id, start)| (*id, *start + offset))
            .collect();
        out.push(Intent::MoveNodes {
            grabbed: node_id,
            to,
        });
    }
}

/// Prepass scan: surface an `OpenGraph` for any subgraph node whose `S`
/// chip was clicked (read from last frame's response). Detecting the
/// open here — *before* the record — lets `App` switch the active graph
/// ahead of Pass A, so the subgraph records a pass earlier and its
/// connections draw with no first-frame gap. Linked subgraphs aren't
/// editable targets yet, so only `Local` opens.
pub(crate) fn emit_subgraph_opens(ui: &Ui, scene: &Scene, actions: &mut Vec<UiAction>) {
    for n in &scene.nodes {
        // Instances are always `Local` (library subgraphs are localized on
        // instance), so the "S" chip opens the interior directly.
        if let Some(SubgraphRef::Local(id)) = n.subgraph
            && ui.response_for(subgraph_badge_wid(n.id)).clicked
        {
            actions.push(UiAction::OpenGraph(GraphRef::Local(id)));
        }
    }
}

/// A click on an `FsPath` input's inline pick button, surfaced for the
/// caller to translate into a deferred file-dialog command. The node UI
/// produces the domain request (node + port + picker config) and stays
/// unaware of the app-level `AppCommand` enum — the canvas, which already
/// owns the command channel, does the translation.
pub(crate) struct PathPickRequest {
    pub(crate) node_id: NodeId,
    pub(crate) port_idx: usize,
    /// The picker config is type-level metadata, taken from the port's
    /// `DataType` (the value only carries the path string).
    pub(crate) config: Arc<FsPathConfig>,
}

/// Scan for a click on an `FsPath` input's inline pick button (polled by
/// its const-editor id, from last frame's responses). Returns the first
/// hit — one pick per frame — for the caller to defer into a blocking file
/// dialog outside the record.
pub(crate) fn emit_path_picks(ui: &Ui, scene: &Scene) -> Option<PathPickRequest> {
    for node in &scene.nodes {
        for (port_idx, input) in scene.inputs(node.inputs).iter().enumerate() {
            if let InputBindingView::Const(StaticValue::FsPath(_)) = &input.binding
                && let DataType::FsPath(config) = &input.ty
                && ui.response_for(const_editor_wid(node.id, port_idx)).clicked
            {
                return Some(PathPickRequest {
                    node_id: node.id,
                    port_idx,
                    config: config.clone(),
                });
            }
        }
    }
    None
}

/// Prepass scan: port double-clicks read from last frame's responses. An
/// input double-click (on the port circle *or* its label) toggles the
/// binding — clears it, or seeds the default const when unbound; an output
/// double-click disconnects every consumer it feeds.
///
/// Emitted pre-record (like the connection commit) because adding or removing
/// a `Const` input's inline editor resizes the node — doing it before Pass A
/// lets the node arrange at its settled size and the wires re-anchor the same
/// frame, instead of floating until the relayout pass.
pub(crate) fn emit_port_dblclicks(ui: &Ui, scene: &Scene, out: &mut Vec<Intent>) {
    for node in &scene.nodes {
        // Boundary ports route the interface — no const affordance, so an
        // unbound one has nothing to seed (its label double-click renames).
        let can_set = !node.boundary;
        for (i, input) in scene.inputs(node.inputs).iter().enumerate() {
            let port = PortRef {
                node_id: node.id,
                kind: PortKind::Input,
                port_idx: i,
            };
            // The circle intercepts its own rect; the cell catches the label.
            let dbl = ui.response_for(port_circle_wid(port)).double_clicked()
                || ui.response_for(input_cell_wid(port)).double_clicked();
            if !dbl {
                continue;
            }
            match &input.binding {
                // Unbound → seed the default literal (or first enum / value-
                // option variant, both already folded into `SceneInput::default`).
                InputBindingView::None => {
                    if can_set && let Some(default) = &input.default {
                        out.push(set_input(port, Binding::Const(default.clone())));
                    }
                }
                // Already bound → clear it.
                _ => out.push(set_input(port, Binding::None)),
            }
        }
        for port in node_ports(node, PortKind::Output) {
            if ui.response_for(port_circle_wid(port)).double_clicked() {
                // An output may feed many inputs — clear each consumer.
                for c in &scene.connections {
                    if c.src_node == port.node_id && c.src_port == port.port_idx {
                        out.push(set_input(
                            PortRef {
                                node_id: c.tgt_node,
                                kind: PortKind::Input,
                                port_idx: c.tgt_port,
                            },
                            Binding::None,
                        ));
                    }
                }
            }
        }
    }
}

/// The accent color for a node's last-run status, or `None` when it
/// didn't run. Shared by the body glow and the header time label so they
/// read as one cue.
pub(crate) fn exec_color(theme: &Theme, status: ExecStatus) -> Option<Color> {
    match status {
        ExecStatus::None => None,
        ExecStatus::Cached => Some(theme.exec_cached_glow),
        ExecStatus::Executed(_) => Some(theme.exec_executed_glow),
        ExecStatus::Running(_) => Some(theme.exec_running_glow),
        ExecStatus::MissingInputs => Some(theme.exec_missing_glow),
        ExecStatus::Errored => Some(theme.exec_errored_glow),
    }
}

/// The status glow for a node's last-run outcome, or `Shadow::NONE`
/// when it didn't run. Zero offset so the halo wraps evenly; a small
/// spread pushes it past the border so it reads at any zoom.
fn exec_shadow(theme: &Theme, status: ExecStatus) -> Shadow {
    match exec_color(theme, status) {
        Some(color) => Shadow {
            color,
            offset: Vec2::ZERO,
            blur: 3.0,
            spread: 0.0,
            inset: false,
        },
        None => Shadow::NONE,
    }
}

/// Stable widget id for the node's outer body panel. Derived from
/// the domain `NodeId` so `response_for` can probe last-frame's
/// arranged rect (used by the connection breaker's body-hit test)
/// without needing the panel's response to round-trip first.
pub(crate) fn node_widget_id(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.body", node_id))
}

/// Stable id for a node's inline title-rename editor (and its idle
/// label), so the same id is recorded across the label⇄editor swap.
/// Polled here to drag the node by its title (the idle label senses
/// `DRAG`) and by [`header::title`] to render the field.
pub(crate) fn node_rename_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.title_rename", node_id))
}

pub(crate) fn set_input(port: PortRef, to: Binding) -> Intent {
    Intent::SetInput {
        node_id: port.node_id,
        input_idx: port.port_idx,
        to,
    }
}

/// The `SetSelection` a click on `node_id` produces: plain click selects
/// only it, Shift-click toggles its membership. Shared by the node body
/// and the port labels so clicking a label selects the node like the
/// body does. `UndoStep::is_noop` drops the entry when nothing changed.
pub(crate) fn select_intent(shift: bool, scene: &Scene, node_id: NodeId) -> Intent {
    let mut to = if shift {
        scene.selected_nodes.clone()
    } else {
        BTreeSet::new()
    };
    if shift && scene.selected_nodes.contains(&node_id) {
        to.remove(&node_id);
    } else {
        to.insert(node_id);
    }
    Intent::SetSelection { to }
}

/// Back-to-front node paint order: most-recently-(committed-)selected nodes
/// last, so they paint on top and *stay* raised until another node is
/// selected. Reconciled against the live node set + selection each frame.
#[derive(Default, Debug)]
struct ZOrder {
    order: Vec<NodeId>,
    /// Last reconcile's selection, to spot nodes newly added to it.
    prev_selected: BTreeSet<NodeId>,
}

impl ZOrder {
    /// Fold this frame's committed `selected` set in — every id newly added
    /// since the last call moves to the back (top) — then reconcile with the
    /// live `current` id set (scene order): drop departed ids, append any not
    /// yet ordered (freshly added) on top. Returns the resulting order.
    fn reconcile(&mut self, current: &[NodeId], selected: &BTreeSet<NodeId>) -> &[NodeId] {
        // Raise nodes newly added to the selection; re-selecting one already
        // on top leaves it there. Iteration is BTreeSet order, which only
        // matters when several commit at once (a rubber band) — recency among
        // simultaneously-selected nodes isn't otherwise defined.
        for id in selected {
            if !self.prev_selected.contains(id) {
                self.order.retain(|z| z != id);
                self.order.push(*id);
            }
        }
        self.prev_selected.clone_from(selected);
        // Reconcile membership: drop departed nodes, then append freshly-added
        // ones on top in scene order. Linear `contains` is fine — a graph
        // holds at most a few hundred nodes.
        self.order.retain(|id| current.contains(id));
        for id in current {
            if !self.order.contains(id) {
                self.order.push(*id);
            }
        }
        &self.order
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<NodeId> {
        (0..n).map(|_| NodeId::unique()).collect()
    }

    #[test]
    fn newly_selected_rises_to_top_and_stays_after_deselect() {
        let n = ids(3);
        let current = n.clone();
        let mut z = ZOrder::default();

        // First frame, nothing selected → order seeds in scene order.
        z.reconcile(&current, &BTreeSet::new());
        assert_eq!(z.order, vec![n[0], n[1], n[2]]);

        // Select n[0] → it rises to the top (back of the list).
        z.reconcile(&current, &BTreeSet::from([n[0]]));
        assert_eq!(
            z.order,
            vec![n[1], n[2], n[0]],
            "selected node moves to top"
        );

        // Deselect → order unchanged: the raise *persists* (recency stack).
        z.reconcile(&current, &BTreeSet::new());
        assert_eq!(
            z.order,
            vec![n[1], n[2], n[0]],
            "stays raised after deselect"
        );

        // Select n[1] → above the previously-raised n[0].
        z.reconcile(&current, &BTreeSet::from([n[1]]));
        assert_eq!(z.order, vec![n[2], n[0], n[1]]);
    }

    #[test]
    fn departed_nodes_drop_and_new_nodes_append_on_top() {
        let n = ids(3);
        let mut z = ZOrder {
            order: vec![n[0], n[1], n[2]],
            prev_selected: BTreeSet::new(),
        };

        // n[1] removed from the graph; a brand-new node appears.
        let fresh = NodeId::unique();
        let current = vec![n[0], n[2], fresh];
        z.reconcile(&current, &BTreeSet::new());
        assert_eq!(
            z.order,
            vec![n[0], n[2], fresh],
            "removed node dropped; new node appended on top",
        );
    }

    #[test]
    fn reselecting_the_top_node_is_idempotent() {
        let n = ids(2);
        let current = n.clone();
        let mut z = ZOrder {
            order: vec![n[0], n[1]],
            // n[1] was already selected last frame.
            prev_selected: BTreeSet::from([n[1]]),
        };

        z.reconcile(&current, &BTreeSet::from([n[1]]));
        assert_eq!(
            z.order,
            vec![n[0], n[1]],
            "no churn when selection is unchanged"
        );
    }
}
