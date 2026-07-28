use glam::Vec2;
use palantir::{CurveBrush, InternedStr, PointerButton, PointerEvent, PointerWake, Ui};
use scenarium::DataType;
use scenarium::{Binding, InputPort, closes_data_cycle};

use crate::core::document::{BoundarySide, PortKind, PortRef};
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::Intent;
use crate::gui::app::AppContext;
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::canvas::wire::{GlyphDrag, Wire, WirePass, WireTint};
use crate::gui::canvas::{outer_canvas_widget_id, preview_drag_modifier};
use crate::gui::node::port_color::port_color;
use crate::gui::node::set_input;
use crate::gui::scene::{GraphScene, InputBindingView, Scene};
use crate::gui::theme::Theme;

/// Owns the in-flight new-connection wire — a held drag or a free-floating
/// wire, see [`InFlight`]. Single-wire-at-a-time means one `Option` is
/// enough. The permanent connections aren't state at all: they live on
/// `Scene` and are painted straight off it by the module-level [`draw`],
/// which needs nothing from here.
#[derive(Default, Debug)]
pub(super) struct ConnectionUI {
    state: Option<InFlight>,
    /// Source port of a wire dropped on empty canvas this frame. Handed to
    /// the new-node popup so it opens; the wire then resumes *floating*
    /// once a node is picked (see [`DragMode::Floating`]). Taken by the
    /// canvas the same frame.
    pending_open: Option<PortRef>,
    /// Set when a floating wire ended on a right-click this frame, so the
    /// canvas can suppress the new-node popup that same right-click would
    /// otherwise open — a right-click then reads purely as "cancel".
    ended_on_secondary: bool,
}

/// The in-flight wire being created: a [`GlyphDrag`] within the data-port
/// domain (both ends are `PortRef`s — a wire can be pulled from either kind),
/// plus which terminating input ends it. Both modes share one preview renderer
/// ([`ConnectionUI::draw_in_flight`]), snap tracking, and data, so `mode` is a
/// discriminant rather than distinct payloads.
#[derive(Clone, Copy, Debug)]
pub(super) struct InFlight {
    pub(super) drag: GlyphDrag<PortRef, PortRef>,
    pub(super) mode: DragMode,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum DragMode {
    /// LMB-drag from the latched port; ends on button release. The original
    /// gesture: a compatible snap commits, an input dropped on its
    /// own body gets a const, a drop on empty canvas opens the palette.
    Held,
    /// Free wire following the cursor with **no button held** — entered
    /// after a dropped wire spawned a node, so the user aims at the exact
    /// port. A left-click over a compatible port commits; a left-click
    /// elsewhere, a right-click, or Esc cancels.
    Floating,
}

impl ConnectionUI {
    /// Drive the in-flight wire: latch a fresh drag, track the snap
    /// target, and resolve on the active mode's terminating input.
    ///
    /// Latch: the first port whose `PortLayer::first_drag_started` fires starts
    /// a [`DragMode::Held`] wire. While active, every port is rescanned each
    /// frame for the topmost opposite-kind port under the pointer
    /// (`drag.snap`). [`DragMode::Held`] resolves on release; a drop on
    /// empty canvas opens the new-node palette instead of dropping, and
    /// `resume` (the source of such a wire, after its node was picked)
    /// re-enters [`DragMode::Floating`] so the user clicks the exact port
    /// to land it. `cancelled` — the frame's Esc, resolved once by the
    /// canvas — drops either mode without emitting anything.
    ///
    /// Swept over the whole scene once per frame. The latch scan spans
    /// every pane (only one press exists), but everything after it runs
    /// against the pane that owns the wire's start node — which is also
    /// what makes a cross-pane wire unrepresentable: the snap scan never
    /// sees another graph's ports.
    pub(super) fn apply(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        geometry: &CanvasGeometry,
        resume: Option<PortRef>,
        cancelled: bool,
        out: &mut Intents,
    ) {
        self.ended_on_secondary = false;

        // A just-spawned node hands its dropped wire back to float.
        if let Some(start) = resume {
            self.state = Some(InFlight {
                drag: GlyphDrag::new(start),
                mode: DragMode::Floating,
            });
        }
        // Latch a fresh port drag only when idle.
        let candidates = drag_candidates(scene, preview_drag_modifier(ui));
        if self.state.is_none()
            && let Some(drag) = GlyphDrag::latch(&geometry.ports, candidates)
        {
            self.state = Some(InFlight {
                drag,
                mode: DragMode::Held,
            });
        }
        if cancelled {
            self.state = None;
        }
        // Both modes span frames, and undo runs before this prepass, so the
        // node the wire grew out of can disappear under it (or its pane can
        // close). Drop the gesture rather than let it keep snapping — a
        // commit against a dead producer is refused at the edit boundary
        // anyway, silently, and `port_data_type` would meanwhile report the
        // start as untyped (which `scan_snap_target` reads as "compatible
        // with anything").
        let Some(mut state) = self.state else {
            return;
        };
        let Some(graph) = scene.owner(state.drag.node()) else {
            self.state = None;
            return;
        };

        // Refresh the compatible port under the pointer for both modes.
        state.drag.snap = scan_snap_target(geometry, ui, graph, state.drag.from);
        self.state = Some(state);

        match state.mode {
            DragMode::Held => self.resolve_held(ui, graph, geometry, state.drag, out),
            DragMode::Floating => self.resolve_floating(ui, graph, state.drag, out),
        }
    }

    /// Take the source port of a wire dropped on empty canvas this frame,
    /// if it started in `graph`'s pane — the canvas hands it to that pane's
    /// new-node popup to open it. Every pane's draw asks, so the ownership
    /// test and the take are one call: a pane that doesn't own the wire
    /// must not consume it.
    pub(super) fn take_pending_connection_in(&mut self, graph: GraphScene<'_>) -> Option<PortRef> {
        let start = self.pending_open?;
        graph.contains(start.node_id).then(|| {
            self.pending_open = None;
            start
        })
    }

    /// Whether a new-connection gesture is in flight **over `graph`'s
    /// pane** — feeds that pane's wire-fade tier. Scoped, so pulling a
    /// wire in one pane doesn't dim the standing wires in every other.
    /// (A method, not a `pub(super)` field: `InFlight` is module-private.)
    pub(super) fn dragging_in(&self, graph: GraphScene<'_>) -> bool {
        self.state
            .is_some_and(|state| graph.contains(state.drag.node()))
    }

    /// Whether a floating wire ended on a right-click this frame — the
    /// canvas suppresses the palette that same right-click would open.
    pub(super) fn ended_on_secondary(&self) -> bool {
        self.ended_on_secondary
    }

    /// `Held` release: commit a snapped port, else set a const on an
    /// input dropped on its own body, else open the new-node palette for a
    /// drop on empty canvas. While the button is still down, keep the wire.
    fn resolve_held(
        &mut self,
        ui: &mut Ui,
        graph: GraphScene<'_>,
        geometry: &CanvasGeometry,
        drag: GlyphDrag<PortRef, PortRef>,
        out: &mut Intents,
    ) {
        if drag.held(&geometry.ports) {
            return;
        }
        if let Some(end) = drag.snap {
            commit_connection(graph, drag.from, end, out);
        } else if let Some(intent) = const_drop(ui, graph, geometry, drag.from) {
            out.push(graph.target(), intent);
        } else if dropped_on_empty_canvas(ui, graph, geometry) {
            // Open the palette and remember the source; the wire resumes
            // floating once a node is picked.
            self.pending_open = Some(drag.from);
        }
        self.state = None;
    }

    /// `Floating` resolve: the wire follows the cursor with no button held,
    /// so its terminating clicks aren't a widget drag — read them off the
    /// global pointer stream (subscribe to wake it; it's empty otherwise).
    /// Left-click lands the wire on a compatible port (or cancels if over
    /// none); right-click cancels (and suppresses the palette); Esc is
    /// handled in `apply`.
    fn resolve_floating(
        &mut self,
        ui: &mut Ui,
        graph: GraphScene<'_>,
        drag: GlyphDrag<PortRef, PortRef>,
        out: &mut Intents,
    ) {
        // `MOVE` wakes a repaint on every cursor move so the wire tracks the
        // pointer (no button is held, so there's no drag-capture keeping
        // frames coming — without this it only redraws when some other
        // widget's hover change happens to wake a frame). `BUTTONS` delivers
        // the terminating press.
        ui.watch_pointer(PointerWake::MOVE | PointerWake::BUTTONS);
        let ended = ui.pointer_events().iter().find_map(|ev| match ev {
            PointerEvent::Down {
                button: button @ (PointerButton::Left | PointerButton::Right),
                ..
            } => Some(*button),
            _ => None,
        });
        match ended {
            Some(PointerButton::Left) => {
                if let Some(end) = drag.snap {
                    commit_connection(graph, drag.from, end, out);
                }
                self.state = None;
            }
            Some(PointerButton::Right) => {
                self.ended_on_secondary = true;
                self.state = None;
            }
            _ => {} // keep floating
        }
    }

    /// Force the hover flag on the port the wire is currently snapped to, so
    /// `port_row` picks the hover color up through the same lookup it uses for
    /// an ordinary mouse-over — palantir suppresses `response.hovered` on
    /// every widget but the drag-capture owner while a drag is live, so the
    /// snapped-but-not-captured target would otherwise stay at its idle color.
    pub(super) fn bake_snap_hover(&self, geometry: &mut CanvasGeometry) {
        if let Some(snap) = self.state.and_then(|state| state.drag.snap) {
            geometry.ports.set_hovered(snap);
        }
    }

    /// Paint the in-flight drag preview: cubic from the start port's
    /// center to either the snapped target's center (when set) or the
    /// pointer position. Drawn inside the inner canvas so coordinates
    /// share the pan/zoom transform with permanent connections.
    pub(super) fn draw_in_flight(
        &self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        graph: GraphScene<'_>,
        geometry: &CanvasGeometry,
        canvas_origin: Vec2,
    ) {
        // Scoped: the preview belongs to the pane holding the wire's
        // start node. Unscoped, every *other* pane also drew it — from
        // its own `canvas_origin` and under its own transform, so the
        // wire's graph-space endpoints landed as a phantom curve over an
        // unrelated graph.
        let Some(state) = self.state.filter(|s| graph.contains(s.drag.node())) else {
            return;
        };
        let start_port = state.drag.from;
        let Some(start) = geometry.ports.center(start_port) else {
            return;
        };
        let Some(end) = state
            .drag
            .free_end(ui, graph, canvas_origin, &geometry.ports)
        else {
            return;
        };
        // Orient handles by kind: outputs grow rightward, inputs grow
        // leftward. Same dx algebra as `draw` so the preview matches
        // the eventual permanent curve exactly when snapped.
        let (p0, p3) = match start_port.kind {
            PortKind::Output => (start, end),
            PortKind::Input => (end, start),
        };
        // Tint the in-flight wire by the dragged port's data type, so the
        // preview already reads as the type being connected.
        let drag_ty = port_data_type(graph, start_port).unwrap_or_default();
        let color = port_color(ctx.theme, &drag_ty, start_port.kind, false);
        Wire::data(p0, p3).add(ui, ctx.theme.connection_width, CurveBrush::Solid(color));
    }
}

/// Paint every permanent connection on the current scene retained by the
/// pass's cull region, marking those the active breaker crosses as broken
/// via `probe.mark_broken_input` for the breaker's release-frame drain.
///
/// Reads nothing but committed scene state off `pass`, so it belongs to the
/// module rather than [`ConnectionUI`] — the in-flight gesture that struct
/// owns has no bearing on how the standing wires paint.
pub(super) fn draw(ui: &mut Ui, pass: &mut WirePass<'_, '_>) {
    let (theme, graph, geometry) = (pass.rcx.theme, pass.rcx.graph, pass.rcx.geometry);
    for c in graph.connections() {
        let src = PortRef {
            node_id: c.src.node_id,
            kind: PortKind::Output,
            port_idx: c.src.port_idx,
        };
        let tgt = PortRef {
            node_id: c.tgt.node_id,
            kind: PortKind::Input,
            port_idx: c.tgt.port_idx,
        };
        let (Some(p0), Some(p3)) = (geometry.ports.center(src), geometry.ports.center(tgt)) else {
            continue;
        };
        let hover = geometry.ports.is_hovered(src) || geometry.ports.is_hovered(tgt);
        let wire = Wire::data(p0, p3);
        if pass.draw_wire(ui, &wire, hover, || data_tint(theme, graph, src, tgt)) {
            pass.probe.mark_broken_input(c.tgt);
        }
    }
}

/// The endpoint colors of a committed data wire: the two ports' type colors,
/// so each end matches the port it touches and the wire reads as its data type
/// (both ends share it unless one side is the untyped `Any` wildcard).
///
/// A wire an upstream wildcard retype left type-mismatched paints entirely in
/// the missing-input warning color instead. Nothing severs it — it flattens as
/// unbound (drift tolerance) — so it wears the same warning the run will report
/// on the port.
fn data_tint(theme: &Theme, graph: GraphScene<'_>, src: PortRef, tgt: PortRef) -> WireTint {
    let src_ty = port_data_type(graph, src).unwrap_or_default();
    let tgt_ty = port_data_type(graph, tgt).unwrap_or_default();
    if !tgt_ty.compatible_with(&src_ty) {
        return WireTint::flat(theme.colors.exec_missing_glow);
    }
    WireTint::new(
        port_color(theme, &src_ty, PortKind::Output, false),
        port_color(theme, &tgt_ty, PortKind::Input, false),
    )
}

/// Every port a fresh wire drag may latch on, a node's inputs before its
/// outputs so the topmost recorded port wins ties (matches paint order).
///
/// The output column drops out while the preview-spawn chord is held
/// ([`preview_drag_modifier`], passed in as `preview_chord` so the returned
/// iterator doesn't keep `Ui` borrowed): that chord is reserved for the
/// preview-spawn drag (see `preview_drag.rs`), so the two controllers never
/// both latch the same press.
fn drag_candidates(scene: &Scene, preview_chord: bool) -> impl Iterator<Item = PortRef> {
    let kinds: &'static [PortKind] = if preview_chord {
        &[PortKind::Input]
    } else {
        &[PortKind::Input, PortKind::Output]
    };
    scene
        .nodes
        .values()
        .flat_map(|n| kinds.iter().flat_map(move |&kind| n.ports(kind)))
}

/// Whether `port` is a const-only input — one that rejects a wired binding, so a
/// dragged wire must never snap to it or start a bind from it.
fn input_const_only(graph: GraphScene<'_>, port: PortRef) -> bool {
    if port.kind != PortKind::Input {
        return false;
    }
    graph
        .node(port.node_id)
        .and_then(|n| graph.inputs(n.inputs).get(port.port_idx))
        .is_some_and(|i| i.const_only)
}

/// Port currently under the pointer that is a compatible target for `start` —
/// opposite kind, a different node, type-compatible, and not cycle-forming.
/// Uses a geometry test against the cached port rect rather than
/// `response.hovered`: palantir suppresses `hovered` on every widget except the
/// LMB-capture owner during a drag, so while the start port owns the capture no
/// other port can ever read `hovered = true`.
fn scan_snap_target(
    geometry: &CanvasGeometry,
    ui: &mut Ui,
    graph: GraphScene<'_>,
    start: PortRef,
) -> Option<PortRef> {
    let pointer = ui.pointer_pos()?;
    // A const-only input rejects wired bindings: a drag that starts on one never
    // snaps anywhere, so its release falls through to the set-const gesture.
    if input_const_only(graph, start) {
        return None;
    }
    // A passthrough (graph input boundary wired straight to the output
    // boundary) leaves the relayed value untyped at execution and panics
    // the worker — disallow it by never snapping one boundary node onto
    // the other. The only boundary→boundary link possible is exactly that
    // passthrough, so a blanket reject is precise.
    let start_boundary = graph.node(start.node_id).is_some_and(|n| n.boundary);
    let candidates = graph
        .nodes()
        .filter(|n| n.id != start.node_id && !(start_boundary && n.boundary))
        .flat_map(|n| n.ports(start.kind.opposite()))
        // A const-only input is never a valid wire target.
        .filter(|&port| !input_const_only(graph, port));
    // Geometrically only one port sits under the pointer, so a port the
    // pointer is over but that `accepts_wire` rejects falls through to
    // `None` (drop) rather than snapping elsewhere.
    geometry
        .ports
        .first_containing(pointer, candidates)
        .filter(|&port| accepts_wire(graph, start, port))
}

/// Whether a wire dragged from `start` may land on `port` — the two
/// rejections that outlive the geometric hit test in [`scan_snap_target`].
fn accepts_wire(graph: GraphScene<'_>, start: PortRef, port: PortRef) -> bool {
    let compatible = match (port_data_type(graph, start), port_data_type(graph, port)) {
        (Some(a), Some(b)) => a.compatible_with(&b),
        // Missing type info (port not in the scene this frame) — don't
        // block; let the intent layer decide.
        _ => true,
    };
    // ...and reject a drop that would close a data-flow cycle: the planner
    // rejects a cyclic graph outright (`CycleDetected`) and the intent layer
    // refuses to commit one, so the wire must never latch. `start.kind` fixes
    // which side is the producer (output) and which the consumer (input).
    // The pane's connection slice is that graph's edge mirror, fed to the
    // same scenarium check the intent layer uses.
    let (producer, consumer) = match start.kind {
        PortKind::Output => (start.node_id, port.node_id),
        PortKind::Input => (port.node_id, start.node_id),
    };
    let edges = graph
        .connections()
        .iter()
        .map(|c| (c.src.node_id, c.tgt.node_id));
    compatible && !closes_data_cycle(edges, producer, consumer)
}

/// "Set const" gesture: an input-port drag released over its own
/// node's body (and not onto a compatible port) means the user
/// wants a literal there. Returns the `SetInput { Const(default) }`
/// intent, or `None` when the gesture doesn't apply — drag started
/// on an output, released off the start node, the port is unknown,
/// or the input is already a const (don't clobber the value).
fn const_drop(
    ui: &mut Ui,
    graph: GraphScene<'_>,
    geometry: &CanvasGeometry,
    start: PortRef,
) -> Option<Intent> {
    if start.kind != PortKind::Input {
        return None;
    }
    let pointer = ui.pointer_pos()?;
    if !geometry.node_screen_rect(start.node_id)?.contains(pointer) {
        return None;
    }
    let node = graph.node(start.node_id)?;
    // Boundary ports route the interface, not literal values.
    if node.boundary {
        return None;
    }
    // Don't overwrite an existing const value.
    let input = graph.inputs(node.inputs).get(start.port_idx)?;
    if matches!(input.binding, InputBindingView::Const(_)) {
        return None;
    }
    let default = input.default.clone()?;
    Some(set_input(start, Binding::Const(default)))
}

/// Whether the pointer is over the canvas but not over any node body —
/// the "released into empty space" condition that offers the new-node
/// palette. Both halves are screen-space, the frame the raw pointer is in;
/// the node half comes off `CanvasGeometry`'s snapshot of this frame's body
/// rects, taken in the same sweep `const_drop` reads.
fn dropped_on_empty_canvas(ui: &mut Ui, graph: GraphScene<'_>, geometry: &CanvasGeometry) -> bool {
    let Some(pointer) = ui.pointer_pos() else {
        return false;
    };
    let over_canvas = ui
        .response_for(outer_canvas_widget_id(graph.target()))
        .rect
        .is_some_and(|r| r.contains(pointer));
    over_canvas && !geometry.over_any_node(pointer)
}

/// The declared [`DataType`] of `port` in the current scene, or `None`
/// if the port isn't present (e.g. mid-rebuild).
fn port_data_type(graph: GraphScene<'_>, port: PortRef) -> Option<DataType> {
    let node = graph.node(port.node_id)?;
    let ty = match port.kind {
        PortKind::Input => graph.inputs(node.inputs).get(port.port_idx)?.ty.clone(),
        PortKind::Output => graph.outputs(node.outputs).get(port.port_idx)?.ty.clone(),
    };
    Some(ty)
}

/// Convert a snapped `(start, end)` PortRef pair (one `Input`, one
/// `Output` — caller-guaranteed by [`scan_snap_target`]) into an
/// `Intent::SetInput` binding. A cycle-forming pair never reaches here —
/// [`scan_snap_target`] refuses to snap one, and `build_step` rejects any
/// cycle-forming bind that slips through (the planner is the final backstop,
/// `Error::CycleDetected`). Re-typing a wildcard output (passthrough / reroute)
/// severs nothing downstream: a now-mismatched wire is tolerated, drawn in
/// the warning color, and flattens as unbound.
///
/// An endpoint that is a boundary node's trailing "+" placeholder first
/// materializes the interface port it stands for (`Intent::AddBoundaryPort`)
/// in the same batch, so add + bind undo as one entry.
fn commit_connection(graph: GraphScene<'_>, start: PortRef, end: PortRef, out: &mut Intents) {
    let (input, output) = match (start.kind, end.kind) {
        (PortKind::Input, PortKind::Output) => (start, end),
        (PortKind::Output, PortKind::Input) => (end, start),
        // Both entries into a commit snap through `scan_snap_target`, which
        // only ever offers `start.kind.opposite()`. A same-kind pair means
        // that invariant broke upstream — not that the user did something
        // odd — and silently dropping it would hide the break behind a wire
        // that just doesn't land.
        (a, b) => unreachable!("a wire committed a {a:?} → {b:?} pair"),
    };
    let target = graph.target();
    out.extend(target, add_boundary_port_intent(graph, output, input));
    out.extend(target, add_boundary_port_intent(graph, input, output));
    out.push(
        target,
        Intent::SetInput {
            input: InputPort::new(input.node_id, input.port_idx),
            to: Some(Binding::bind(output.node_id, output.port_idx)),
        },
    );
}

/// When `port` is a boundary node's trailing placeholder, the
/// `Intent::AddBoundaryPort` that materializes its interface slot. Named
/// `input{N}`/`output{N}` (first free N over the node's existing ports)
/// and typed from `opposite` — the placeholder itself is untyped; the
/// other endpoint carries the type (a consumer's declared input type, a
/// producer's resolved output type). `None` for ordinary ports.
fn add_boundary_port_intent(
    graph: GraphScene<'_>,
    port: PortRef,
    opposite: PortRef,
) -> Option<Intent> {
    let node = graph.node(port.node_id)?;
    if !node.boundary {
        return None;
    }
    // A boundary node mirrors the interface: the `GraphInput` node's
    // *output* column is the graph's `Input` side and vice versa.
    let (side, prefix) = match port.kind {
        PortKind::Output => (BoundarySide::Input, "input"),
        PortKind::Input => (BoundarySide::Output, "output"),
    };
    // The two columns hold different row types, so each kind reads its own —
    // but reads it once, and both hand back the same answer.
    let taken = match port.kind {
        PortKind::Output => taken_suffixes(
            graph.outputs(node.outputs).iter().map(|o| &o.name),
            port.port_idx,
            prefix,
        ),
        PortKind::Input => taken_suffixes(
            graph.inputs(node.inputs).iter().map(|i| &i.name),
            port.port_idx,
            prefix,
        ),
    }?;
    Some(Intent::AddBoundaryPort {
        side,
        name: fresh_port_name(prefix, taken),
        data_type: port_data_type(graph, opposite).unwrap_or_default(),
    })
}

/// The `{prefix}{n}` numbers the ports *before* `idx` already use, or `None`
/// when `idx` isn't the column's trailing "+" placeholder.
///
/// Takes the names rather than the rows because a boundary node's two columns
/// hold different row types and this is the only thing either is asked for.
/// A name that isn't the prefix followed by a decimal number can't collide
/// with a generated one, so it contributes nothing and is dropped here.
fn taken_suffixes<'a>(
    names: impl ExactSizeIterator<Item = &'a InternedStr>,
    idx: usize,
    prefix: &str,
) -> Option<Vec<usize>> {
    if idx + 1 != names.len() {
        return None; // an existing interface port, not the trailing placeholder
    }
    Some(
        names
            .take(idx)
            .filter_map(|name| {
                let text = name.borrow_str();
                text.strip_prefix(prefix)?.parse().ok()
            })
            .collect(),
    )
}

/// The lowest free `{prefix}{n}` over the already-`taken` suffixes.
///
/// Resolved on the numbers alone: a generated name collides only with the
/// prefix followed by a number, so sorting the taken ones and walking to the
/// first gap answers it without building a candidate `String` per probe or
/// comparing a single pair of strings. Duplicates in `taken` are tolerated —
/// the walk steps past a repeat rather than counting it twice.
fn fresh_port_name(prefix: &str, mut taken: Vec<usize>) -> String {
    taken.sort_unstable();
    let mut free = 0;
    for n in taken {
        if n > free {
            break; // the run of used numbers ends before `free`
        }
        if n == free {
            free += 1;
        }
    }
    format!("{prefix}{free}")
}

#[cfg(test)]
mod tests;
