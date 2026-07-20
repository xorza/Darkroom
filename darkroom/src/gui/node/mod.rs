pub(crate) mod header;
pub(crate) mod memory_row;
pub(crate) mod port_color;
pub(crate) mod port_rename;
pub(crate) mod port_row;
pub(crate) mod prepass;
pub(crate) mod value_editor;

use crate::core::document::ItemRef;
use crate::core::document::PortRef;
use crate::core::edit::intent::types::Intent;
use crate::gui::canvas::breaker::BreakerProbe;
use crate::gui::canvas::cull::CullRegion;
use crate::gui::canvas::drag_anchor::GroupDragAnchor;
use crate::gui::canvas::drag_anchor::selected_group_positions;
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::canvas::inspector::Inspectors;
use crate::gui::canvas::pin_ui::PinUi;
use crate::gui::node::header::{header, status_row, subscription_pin};
use crate::gui::node::memory_row::memory_row;
use crate::gui::node::port_row::ports_row;
use crate::gui::run_state::{ExecStatus, RunState};
use crate::gui::scene::{Scene, SceneNode};
use crate::gui::theme::Theme;
use aperture::{
    Background, Color, Configure, Corners, Panel, Sense, Shadow, Sizing, Stroke, Ui, WidgetId,
};
use glam::Vec2;
use scenarium::Binding;
use scenarium::InputPort;
use scenarium::Library;
use scenarium::NodeId;
use scenarium::OutputPort;
use std::collections::BTreeSet;

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
    /// Effective selection to paint: the committed set (`scene.selected`)
    /// or, mid-rubber-band, the live swept preview owned by `SelectionUI`.
    /// Kept off `Scene` so the projection stays a read-only mirror — the
    /// gesture no longer scribbles its preview into the committed field.
    pub(crate) selected: &'a BTreeSet<ItemRef>,
    pub(crate) geometry: &'a CanvasGeometry,
    /// Open inspection panels, so the header chip can render its
    /// open/pinned state.
    pub(crate) inspectors: &'a Inspectors,
    /// Live run results — the pin previews drawn interleaved with the
    /// node bodies read their pinned values from here.
    pub(crate) run_state: &'a RunState,
}

/// Owns rendering of every graph node plus the single active drag
/// anchor — the press-frame positions are snapshotted here so each
/// `MoveSelection` target is `start_pos + drag_delta`, not a running
/// integration over the moving source. Only one node can hold the
/// pointer at a time, so one anchor slot is enough.
///
/// `draw_all` is the single entry point; `GraphUI` calls it once per
/// frame after [`crate::gui::canvas::geometry::CanvasGeometry`] has been rebuilt
/// from last-frame's responses.
#[derive(Default, Debug)]
pub(crate) struct NodeUI {
    drag_anchor: Option<DragAnchor>,
    /// The node kept recorded by the focus cull-exemption last frame.
    /// Focus clears during input, *before* the record, so on the blur
    /// frame `focus_within` is already false — but that frame is exactly
    /// when an in-progress const edit commits (the editor's pending draft
    /// resolves on its first post-blur record). One frame of hysteresis
    /// keeps the node recorded through it; otherwise the cull would let
    /// aperture sweep the draft unseen.
    focus_kept_last: Option<NodeId>,
}

/// A node-body drag anchor: `key` is the grabbed node's id, always present
/// in `start_positions`. Captured from the `drag_started` frame's
/// `Response::widget_id()` so subsequent frames can `ui.response_for
/// (widget_id)` *before* recording and bake the current `drag_delta` into
/// `.position(...)` — letting the node paint at the cursor's location in
/// Pass A directly, with no need to wait for Pass B's relayout to catch up.
type DragAnchor = GroupDragAnchor<NodeId>;

impl NodeUI {
    /// Record the widget tree of every scene item — node bodies and
    /// pinned-output preview widgets interleaved — retained by `cull`
    /// (plus the focus-owning node — see the loop comment),
    /// skipping off-screen ones entirely. Emits selection/raise intents
    /// for body clicks and latches the drag anchor for a body/title drag
    /// (port circles capture their own presses via `Sense::CLICK`, so
    /// drags don't latch off the port grabs); `prepass` converts the
    /// anchor into `Intent::MoveSelection` on later frames.
    pub(crate) fn draw_all(
        &mut self,
        ui: &mut Ui,
        rcx: RecordCtx<'_>,
        cull: CullRegion,
        probe: &mut BreakerProbe<'_>,
        pin_ui: &mut PinUi,
        out: &mut Vec<Intent>,
    ) {
        // Paint in `scene.z_order` (mirrored from `item_placements`) — later
        // draws sit on top, so the last item in the list is frontmost, and
        // a pin preview can sit above or below any node body. The order is
        // persisted view state, so a raised item stays raised across
        // save/load and tab switches; `Intent::Raise` moves a clicked item
        // to the end. `RecordCtx` is `Copy`, so the `&scene` borrows held
        // by the loop coexist with copying `rcx` into the draw calls.
        //
        // Culled nodes are skipped entirely — no measure, arrange, or
        // paint. Every widget id in a node's subtree derives from its
        // `NodeId` (explicit `from_hash` ids, and aperture resolves auto ids
        // parent-scoped under them), so culling a sibling can't re-key
        // anything that stays on screen. Aperture *does* drop widget state
        // for ids not recorded this frame, so a node whose subtree holds the
        // keyboard focus (`focus_within` — an in-progress title/const/port
        // edit) stays recorded even off-screen; otherwise panning away
        // mid-edit would discard the draft. The exemption carries one frame
        // past the blur (`focus_kept_last`): focus clears before the record,
        // and that first post-blur record is where the edit's pending draft
        // commits.
        let mut focus_kept = None;
        for key in &rcx.scene.z_order {
            let id = match *key {
                ItemRef::Node(id) => id,
                ItemRef::Pin(port) => {
                    // Pin previews hold no keyboard focus, so they need no cull exemption.
                    pin_ui.draw_pin(ui, rcx, port, cull, probe, out);
                    continue;
                }
            };
            let Some(n) = rcx.scene.nodes.get(&id) else {
                continue;
            };
            let keeps_focus = ui.focus_within(node_widget_id(n.id));
            if keeps_focus {
                focus_kept = Some(n.id);
            }
            if !cull.keeps_node(rcx.geometry.node_world_rect(n))
                && !keeps_focus
                && self.focus_kept_last != Some(n.id)
            {
                continue;
            }
            self.draw_one(ui, rcx, n, probe, out);
        }
        self.focus_kept_last = focus_kept;
        // Drop the anchor if its target node vanished from the graph
        // (mid-drag delete). Without this, the slot would linger and
        // could fire when a fresh node reused the id.
        if let Some(a) = &self.drag_anchor
            && !rcx.scene.nodes.contains_key(&a.key)
        {
            self.drag_anchor = None;
        }
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
            .map(|r| probe.to_world(r));
        let broken = body_rect.is_some_and(|r| probe.crosses_rect(r));
        if broken {
            probe.mark_broken_node(node.id);
        }
        let selected = rcx.selected.contains(&ItemRef::Node(node.id));
        // The border width is *always* the selection width so selecting a
        // node never resizes it (stroke folds into padding — width-gated,
        // not color-gated). Only the color changes, a 4-tier decision: the
        // breaker alarm wins, then the missing-stub color, then
        // `Theme::card_border`'s own broken/selected/resting 3-tier (broken
        // can't recur here since it's already handled, but the helper still
        // carries the shape node body and pin preview share).
        let border_width = theme.card_border_width();
        let border = if node.missing && !broken {
            // A stub for a node whose func is gone from the library: paint it
            // in the error color so it reads as broken-but-deletable.
            theme.colors.exec_errored_glow
        } else {
            theme.card_border(broken, selected).color
        };
        // Sample modifiers before the panel borrows `ui` for the rest
        // of this scope (the click handler below can't reborrow it).
        let shift_click = ui.modifiers().shift;
        // Status glow when the node ran, else the ambient elevation shadow —
        // one slot, and live status outranks depth.
        let shadow = node_shadow(theme, node.exec_status);

        // The subscription pin records just before the body so it peeks out
        // from behind this node's corner while riding the same cull decision
        // and stack position as the node itself.
        if node.sink {
            subscription_pin(ui, theme, node, rcx.geometry.subs.is_hovered(node.id));
        }

        let panel = Panel::vstack()
            .id(node_widget_id(node.id))
            .position(node.pos)
            .min_size((theme.node_min_width, theme.node_min_height))
            .size((Sizing::HUG, Sizing::HUG))
            .sense(Sense::CLICK | Sense::DRAG)
            .background(
                Background::rounded(
                    theme.colors.node_fill,
                    Corners::all(theme.node_corner_radius),
                )
                .with_stroke(Stroke::solid(border, border_width))
                .with_shadow(shadow),
            )
            .show(ui, |ui| {
                header(ui, rcx, node, out);
                status_row(ui, rcx, node, out);
                ports_row(ui, rcx, node, out);
                memory_row(ui, rcx, node);
            });
        // Pull the body response's flags into locals so its `&mut ui`
        // borrow ends before the `response_for(title)` peek below.
        let response = panel.response;
        let body_clicked = response.left.clicked();
        let body_drag_started = response.left.drag.started();
        let body_wid = response.id;

        // Click without drag → select. Plain click selects only this
        // node; Shift-click toggles its membership in the current
        // selection. `UndoStep::is_noop` filters a click that doesn't
        // change the set (e.g. clicking the sole selected node).
        if body_clicked {
            click_intents(shift_click, rcx.scene, ItemRef::Node(node.id), out);
        }

        // The header title doubles as a drag handle: its idle label
        // senses `DRAG`, so a drag latched there moves the node like a
        // body drag (the title swallows the press, so the body never sees
        // it). `response_for` is last-frame, matching how `prepass` reads
        // the delta. While renaming the title is a `TextEdit` (no `DRAG`),
        // so this can't fire mid-edit.
        let title_wid = node_rename_wid(node.id);
        let title_drag = ui.response_for(title_wid).left.drag.started();

        // Latch the anchor on the press-frame edge; subsequent frames'
        // `prepass` peeks `response_for(widget_id)` before record runs
        // and converts `drag_delta` into a `MoveSelection` applied to
        // `Document` upstream of `Scene::rebuild`.
        if title_drag || body_drag_started {
            // Grabbing a node already in the selection drags the whole
            // group (nodes and pinned-output previews alike) together;
            // grabbing an unselected node selects only it and drags it
            // alone.
            let start_positions = if selected {
                selected_group_positions(rcx.scene, rcx.selected)
            } else {
                click_intents(false, rcx.scene, ItemRef::Node(node.id), out);
                vec![(ItemRef::Node(node.id), node.pos)]
            };
            self.drag_anchor = Some(DragAnchor {
                key: node.id,
                start_positions,
                widget_id: if title_drag { title_wid } else { body_wid },
            });
        }
    }

    /// Pre-record pass: peek aperture's input state for any widgets
    /// this `NodeUI` owns and push the corresponding `Intent`s into
    /// `out`. Runs before `Scene::rebuild` in `App::record`, so any
    /// state mutation applied from these intents (notably drag-driven
    /// `MoveSelection`) lands in `Document` before recording — Pass A's
    /// arrange already reflects the cursor; no Pass B relayout retry.
    pub(crate) fn prepass(&mut self, ui: &Ui, scene: &Scene, out: &mut Vec<Intent>) {
        // `key`/`widget_id` are `Copy`, so pull them out and drop the
        // borrow — that lets the early returns below reassign
        // `self.drag_anchor` without cloning the `start_*_positions` `Vec`s,
        // which are only read in the success path (where the anchor isn't
        // cleared and can be re-borrowed).
        let Some(&DragAnchor { key, widget_id, .. }) = self.drag_anchor.as_ref() else {
            return;
        };
        // Drop a stale anchor whose node was removed last frame (e.g.
        // breaker swipe deleted the dragged node). Without this, the
        // emitted `MoveSelection` would target a missing node and panic in
        // `build_step`. `draw_all` also clears stale anchors, but only
        // after this prepass runs.
        if !scene.nodes.contains_key(&key) {
            self.drag_anchor = None;
            return;
        }
        let resp = ui.response_for(widget_id);
        // `drag_started` on a still-active anchor means a *new* gesture
        // just latched on the same widget — `record` will replace the
        // anchor this frame; emitting now with the stale start positions
        // makes the nodes snap to the previous gesture's start point.
        if resp.left.drag.started() {
            self.drag_anchor = None;
            return;
        }
        // No `drag_delta` means the drag isn't latched anymore (release
        // or pointer-left-surface). Drop the anchor so the next gesture
        // starts fresh.
        let Some(delta) = resp.left.drag.delta() else {
            self.drag_anchor = None;
            return;
        };
        // Aperture reports drag deltas in the widget's pre-transform frame,
        // which is the same canvas-world coordinate space as node positions.
        // Anchor still present (success path never cleared it); re-borrow
        // to read the start positions without cloning.
        let anchor = self.drag_anchor.as_ref().unwrap();
        out.push(anchor.resolve(delta, ItemRef::Node(key)));
    }
}

/// The accent color for a node's last-run status, or `None` when it
/// didn't run. Shared by the body glow and the header time label so they
/// read as one cue.
pub(crate) fn exec_color(theme: &Theme, status: ExecStatus) -> Option<Color> {
    match status {
        ExecStatus::None => None,
        ExecStatus::Cached => Some(theme.colors.exec_cached_glow),
        ExecStatus::Executed(_) => Some(theme.colors.exec_executed_glow),
        ExecStatus::Running(_) => Some(theme.colors.exec_running_glow),
        ExecStatus::MissingInputs => Some(theme.colors.exec_missing_glow),
        ExecStatus::Errored => Some(theme.colors.exec_errored_glow),
    }
}

/// The node body's one shadow: the status glow for its last-run outcome
/// (zero offset so the halo wraps evenly), or — when it didn't run — a soft
/// ambient drop shadow that lifts the body off the canvas and the wires
/// crossing beneath it. The ambient color is the theme's elevation swatch
/// (`node_ambient_shadow`), shared with the inspector panels so all
/// elevated surfaces cast one kind of shadow.
fn node_shadow(theme: &Theme, status: ExecStatus) -> Shadow {
    match exec_color(theme, status) {
        // Blur/spread sized so the glow carries elevation too — it replaces
        // the ambient shadow, and a tighter halo would leave a just-run node
        // sitting flatter than its idle neighbors. Kept a touch tighter than
        // the ambient shadow so the status reads as a crisp halo, not a bloom.
        Some(color) => Shadow::drop(color, Vec2::ZERO, 3.0).with_spread(0.5),
        None => theme.elevation_shadow(10.0),
    }
}

/// Stable widget id for the node's outer body panel. Derived from
/// the domain `NodeId` so `response_for` can probe last-frame's
/// arranged rect (used by the connection breaker's body-hit test)
/// without needing the panel's response to round-trip first.
pub(crate) fn node_widget_id(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.body", node_id))
}

/// Pointer-over-node for hover-reveal affordances (the value-editor
/// chips). The body response's own `hovered` flag misses most of the
/// node's area — ports, chips, and editors capture the hit — so this
/// asks whether the hover *target* sits anywhere in the node's subtree.
/// Target-derived (not a raw `pointer_pos` rect test) on purpose: it
/// can only change when the hover target changes, which is exactly when
/// a repaint is already scheduled — no `MOVE` subscription needed — and
/// it's occlusion-aware (a panel stacked over the node wins the
/// pointer).
pub(crate) fn node_hovered(ui: &Ui, node_id: NodeId) -> bool {
    ui.hover_within(node_widget_id(node_id))
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
        input: InputPort::new(port.node_id, port.port_idx),
        to,
    }
}

/// Toggle (or set) whether an output port is pinned.
pub(crate) fn set_output_pinned(port: PortRef, pinned: bool) -> Intent {
    Intent::SetOutputPinned {
        output: OutputPort::new(port.node_id, port.port_idx),
        pinned,
    }
}

/// The intents a click on `key` produces: the selection change plus a lift
/// to the top of the shared paint stack, so clicking a node body or a pin
/// preview brings it to the front. The raise is skipped only when a
/// Shift-click *removes* the item from the selection — an item you just
/// deselected shouldn't jump forward. Shared by the node body, header
/// title, and port labels so clicking any of them behaves like clicking the
/// body; also shared by the pin preview widget's own click.
pub(crate) fn click_intents(shift: bool, scene: &Scene, key: ItemRef, out: &mut Vec<Intent>) {
    out.push(select_intent(shift, scene, key));
    let deselecting = shift && scene.selected.contains(&key);
    if !deselecting {
        out.push(Intent::Raise { key });
    }
}

/// The `SetSelection` a click on `key` produces: plain click selects only
/// it, Shift-click toggles its membership. `UndoStep::is_noop` drops the
/// entry when nothing changed.
fn select_intent(shift: bool, scene: &Scene, key: ItemRef) -> Intent {
    let mut to = if shift {
        scene.selected.clone()
    } else {
        BTreeSet::new()
    };
    if shift && scene.selected.contains(&key) {
        to.remove(&key);
    } else {
        to.insert(key);
    }
    Intent::SetSelection { to }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene_with_selection(selected: impl IntoIterator<Item = NodeId>) -> Scene {
        Scene {
            selected: selected.into_iter().map(ItemRef::Node).collect(),
            ..Default::default()
        }
    }

    fn click(shift: bool, scene: &Scene, id: NodeId) -> Vec<Intent> {
        let mut out = Vec::new();
        click_intents(shift, scene, ItemRef::Node(id), &mut out);
        out
    }

    #[test]
    fn click_intents_raises_unless_shift_deselects() {
        let a = NodeId::unique();
        let b = NodeId::unique();

        // Plain click on an unselected node: select it, then raise it.
        let out = click(false, &scene_with_selection([]), a);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], Intent::SetSelection { .. }));
        assert!(matches!(out[1], Intent::Raise { key } if key == ItemRef::Node(a)));

        // Plain click on an already-selected node still raises it.
        let out = click(false, &scene_with_selection([a]), a);
        assert!(
            out.iter()
                .any(|i| matches!(i, Intent::Raise { key } if *key == ItemRef::Node(a))),
            "a plain click always lifts its node to the front"
        );

        // Shift-click adding a fresh node to the selection raises it.
        let out = click(true, &scene_with_selection([a]), b);
        assert!(
            out.iter()
                .any(|i| matches!(i, Intent::Raise { key } if *key == ItemRef::Node(b))),
            "shift-adding a node raises it"
        );

        // Shift-click removing a node does NOT raise it — a node you just
        // deselected shouldn't jump to the front.
        let out = click(true, &scene_with_selection([a, b]), b);
        assert_eq!(out.len(), 1, "shift-deselect suppresses the raise");
        assert!(matches!(out[0], Intent::SetSelection { .. }));
    }

    #[test]
    fn click_intents_raises_a_pin_like_a_node() {
        // A pin preview owns a slot in the same paint stack as node
        // bodies — clicking it selects it *and* lifts it to the front.
        let port = scenarium::OutputPort::new(NodeId::unique(), 0);
        let key = ItemRef::Pin(port);
        let mut out = Vec::new();
        click_intents(false, &Scene::default(), key, &mut out);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], Intent::SetSelection { .. }));
        assert!(matches!(out[1], Intent::Raise { key: k } if k == key));
    }
}
