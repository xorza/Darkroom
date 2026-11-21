use palantir::Ui;
use scenarium::{Binding, Boundary, CompoundId, GraphId, InputPort, Interface, OutputPort, PortId};

use crate::core::edit::compound_intent::CompoundIntent;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::port_edit::{DataSide, PortEdit};
use crate::gui::frame::requests::Requests;
use crate::gui::frame::stage::Stage;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::graph_ctx::input_ctx::InputCtx;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::draw::wire::WirePaint;
use crate::gui::pane::graph_ui::draw_ctx::DrawCtx;
use crate::gui::pane::graph_ui::gesture::glyph_drag::GlyphDrag;
use crate::gui::pane::graph_ui::gesture::preview_drag;
use crate::gui::pane::graph_ui::scene::canvas_geometry::CanvasGeometry;
use crate::gui::pane::graph_ui::scene::wire::Wire;
use crate::gui::port_ref::{PortKind, PortRef};

/// Owns the in-flight new wire, a held drag (see [`GlyphDrag`]); one `Option` is enough. The
/// permanent wires live on the authoring graph and are painted off it by
/// [`WirePass::draw_data_wires`](crate::gui::pane::graph_ui::draw::wire::WirePass::draw_data_wires).
#[derive(Default, Debug)]
pub(crate) struct WireUi {
    /// A drag within the data-port domain: both ends are `PortRef`s, as a wire can be pulled from
    /// either kind. It ends on the button's release: a compatible snap commits, an input dropped
    /// on its own body gets a const, and a drop on empty canvas opens the palette on it.
    state: Option<GlyphDrag<PortRef, PortRef>>,
}

impl WireUi {
    /// Drop the in-flight wire.
    pub(crate) const fn reset(&mut self) {
        self.state = None;
    }

    /// Whether a wire is in flight.
    pub(crate) const fn in_flight(&self) -> bool {
        self.state.is_some()
    }

    /// Drive the in-flight wire: latch a fresh drag, track the snap target, and resolve on the
    /// button's release.
    ///
    /// Latch: the first port whose `PortLayer::first_drag_started` fires starts a wire. While it
    /// is held, every port is rescanned each frame for the topmost opposite-kind port under the
    /// pointer (`drag.snap`). On release a compatible snap commits, a drop on a boundary node
    /// exposes the port, an input dropped on its own body gets a const, and a drop on empty
    /// canvas returns the port the wire grew out of, which the palette opens on. The context's
    /// Esc drops the wire without emitting anything.
    pub(crate) fn input(
        &mut self,
        ui: &mut Ui,
        stage: &mut Stage<'_, CanvasCtx<'_>>,
    ) -> Option<PortRef> {
        let ctx = stage.ctx;
        let (graph_ctx, geometry) = (ctx.graph_ctx(), ctx.geometry());
        // Latch a fresh port drag only when idle.
        // The output column drops out while the preview-spawn chord is
        // held: that chord is the preview drag's, so the two controllers
        // never both latch one press.
        let preview_chord = preview_drag::modifier_held(ui);
        if self.state.is_none()
            && let Some(drag) = GlyphDrag::latch(&geometry.ports, |port| {
                !preview_chord || port.kind == PortKind::Input
            })
            && graph_ctx.contains(drag.node())
        {
            self.state = Some(drag);
        }
        if ctx.cancelled() {
            self.state = None;
        }
        let mut drag = self.state.take()?;
        // A drag spans frames, and undo runs before this input stage, so the node the wire grew
        // out of can be deleted under it. The gesture drops rather than re-latching; a commit
        // against a dead producer is refused at the edit boundary anyway.
        if !graph_ctx.contains(drag.node()) {
            return None;
        }
        drag.snap = scan_snap_target(ui, ctx, drag.from);
        if drag.held(&geometry.ports) {
            self.state = Some(drag);
            return None;
        }
        let out = &mut *stage.out;
        if let Some(end) = drag.snap {
            out.push_graph(graph_ctx.graph(), wire(drag.from, end));
            return None;
        }
        if let Some(exposed) = exposed_on_boundary(ui, ctx, drag.from) {
            exposed.raise(graph_ctx.graph(), out);
            return None;
        }
        if let Some(set_const) = const_drop(ui, ctx, drag.from) {
            out.push_graph(graph_ctx.graph(), set_const);
            return None;
        }
        dropped_on_empty_canvas(ui, graph_ctx.graph(), geometry).then_some(drag.from)
    }

    /// Force the hover flag on the port the wire is currently snapped to, so `port_row` picks
    /// the hover color up as for an ordinary mouse-over: palantir suppresses `response.hovered`
    /// on every widget but the drag-capture owner while a drag is live.
    pub(crate) fn bake_snap_hover(&self, geometry: &mut CanvasGeometry) {
        if let Some(snap) = self.state.as_ref().and_then(|drag| drag.snap) {
            geometry.ports.set_hovered(snap);
        }
    }

    /// Paint the in-flight drag preview: a cubic from the start port's center to the snapped
    /// target's center (when set) or the pointer position. Drawn inside the inner canvas, so
    /// coordinates share the pan/zoom transform with permanent wires.
    pub(crate) fn record(&self, ui: &mut Ui, ctx: DrawCtx<'_>) {
        let (graph_ctx, geometry) = (ctx.graph_ctx(), ctx.geometry());
        let Some(drag) = self.state else {
            return;
        };
        let start_port = drag.from;
        let Some(start) = geometry.ports.center(start_port) else {
            return;
        };
        let pointer = canvas::world_pointer(ui, graph_ctx.graph(), graph_ctx.camera());
        let Some(end) = drag.free_end(pointer, &geometry.ports) else {
            return;
        };
        // Orient by kind (`p0` the output end), so the preview matches the eventual permanent
        // curve when snapped.
        let (p0, p3) = match start_port.kind {
            PortKind::Output => (start, end),
            PortKind::Input => (end, start),
        };
        // Tinted by the dragged port's data type.
        let theme = graph_ctx.theme();
        let drag_data_type = graph_ctx.data_type(start_port);
        let color = graph_ctx.port_color(drag_data_type);
        let width = theme.canvas.wire_at(ctx.zoom());
        WirePaint::from(color).stroke(ui, &Wire::new(p0, p3), width, None);
    }
}

/// Whether `port` is a const-only input, one that rejects a wired binding, so a dragged wire
/// never snaps to it or starts a bind from it.
fn input_const_only(graph_ctx: GraphCtx<'_>, port: PortRef) -> bool {
    port.input_port()
        .and_then(|input| graph_ctx.node(input.node_id)?.input(input.port))
        .is_some_and(InputCtx::const_only)
}

/// Port currently under the pointer that is a compatible target for `start`: opposite kind, a
/// different node, type-compatible, and not cycle-forming. Uses a geometry test against the
/// cached port rect rather than `response.hovered`, which palantir suppresses on every widget
/// except the LMB-capture owner during a drag.
fn scan_snap_target(ui: &mut Ui, ctx: CanvasCtx<'_>, start: PortRef) -> Option<PortRef> {
    let (graph_ctx, geometry) = (ctx.graph_ctx(), ctx.geometry());
    let pointer = ui.pointer_pos()?;
    // A drag that starts on a const-only input never snaps anywhere, so its release falls
    // through to the set-const gesture.
    if input_const_only(graph_ctx, start) {
        return None;
    }
    let candidates = graph_ctx
        .nodes()
        .filter(|n| n.node_id != start.node_id)
        .flat_map(|n| n.ports(start.kind.opposite()));
    // Geometrically only one port sits under the pointer, so one that `accepts_wire` rejects
    // yields `None` (drop) rather than snapping elsewhere.
    geometry
        .ports
        .first_containing(pointer, candidates)
        .filter(|&port| accepts_wire(graph_ctx, start, port))
}

/// Whether a wire dragged from `start` may land on `port`: the edit gate's question,
/// [`Graph::admit_bind`](scenarium::Graph::admit_bind), asked of the same authoring graph, so the
/// snap filter and `GraphIntent::into_step` cannot answer differently. Both ends are live graph
/// nodes: the wire drops once its start is gone.
fn accepts_wire(graph_ctx: GraphCtx<'_>, start: PortRef, port: PortRef) -> bool {
    // `start.kind` fixes which side is the producer (output) and which the
    // consumer (input).
    let (producer, consumer) = match start.kind {
        PortKind::Output => (start, port),
        PortKind::Input => (port, start),
    };
    let input = consumer.input_port().expect("a wire lands on an input");
    let output = producer.output_port().expect("a wire reads an output");
    graph_ctx
        .body()
        .admit_bind(graph_ctx.scope(), input, &Binding::Wire(output))
        .is_ok()
}

/// A wire dropped on a compound's boundary node of the side that takes it: a port the interface
/// gains, read off the end the wire grew out of, and the wire to it. Both land in one batch, so
/// one history entry.
#[derive(Debug)]
struct Exposed {
    compound: CompoundId,
    to: Interface,
    wire: GraphIntent,
}

impl Exposed {
    fn raise(self, graph: GraphId, out: &mut Requests) {
        out.push_compound(CompoundIntent::SetInterface {
            id: self.compound,
            to: Box::new(self.to),
        });
        out.push_graph(graph, self.wire);
    }
}

/// The port a held wire from `start` adds when released over a boundary node of a compound's
/// pane: an output dropped on the `Outputs` node becomes an interface output of its type and
/// name, wired from it; an input dropped on the `Inputs` node becomes an interface input of its
/// declaration, wired to it. `None` off a boundary node, or on the side that does not take it.
fn exposed_on_boundary(ui: &mut Ui, ctx: CanvasCtx<'_>, start: PortRef) -> Option<Exposed> {
    let (graph_ctx, geometry) = (ctx.graph_ctx(), ctx.geometry());
    let GraphId::Compound(compound) = graph_ctx.graph() else {
        return None;
    };
    let pointer = ui.pointer_pos()?;
    let wanted = match start.kind {
        PortKind::Output => Boundary::Outputs,
        PortKind::Input => Boundary::Inputs,
    };
    let body = graph_ctx.body();
    // A compound may hold several `Inputs` nodes; any of them takes the drop.
    let boundary = body
        .iter()
        .filter(|node| node.boundary() == Some(wanted) && node.node_id != start.node_id)
        .find(|node| {
            geometry
                .node_screen_rect(node.node_id)
                .is_some_and(|rect| rect.contains(pointer))
        })?
        .node_id;
    let decl = body.find(start.node_id)?.decl(graph_ctx.scope());
    let interface = &graph_ctx.document().graphs.compound(compound)?.interface;
    let (edit, wire) = match start.kind {
        PortKind::Output => {
            let output = decl.ports().output(start.port)?;
            let port = PortId::mint(|id| interface.ports().output(id).is_some());
            (
                PortEdit::Add {
                    side: DataSide::Outputs,
                    port,
                    name: output.name.clone(),
                    data_type: output.data_type,
                },
                GraphIntent::SetInput {
                    input: InputPort::new(boundary, port),
                    to: Some(Binding::Wire(OutputPort::new(start.node_id, start.port))),
                },
            )
        }
        PortKind::Input => {
            let input = decl.ports().input(start.port)?;
            // A const-only input takes no wire, so no port of the interface could feed it.
            if input.const_only {
                return None;
            }
            let port = PortId::mint(|id| interface.ports().input(id).is_some());
            (
                PortEdit::Add {
                    side: DataSide::Inputs,
                    port,
                    name: input.name.clone(),
                    data_type: input.data_type,
                },
                GraphIntent::SetInput {
                    input: InputPort::new(start.node_id, start.port),
                    to: Some(Binding::Wire(OutputPort::new(boundary, port))),
                },
            )
        }
    };
    Some(Exposed {
        compound,
        to: edit.apply(interface),
        wire,
    })
}

/// "Set const" gesture: an input-port drag released over its own node's body (and not onto a
/// compatible port) means the user wants a literal there. Returns the
/// `SetInput { Const(default) }` intent, or `None` when the gesture doesn't apply: drag started
/// on an output, released off the start node, the port is unknown, or the input is already a
/// const (which is not clobbered).
fn const_drop(ui: &mut Ui, ctx: CanvasCtx<'_>, start: PortRef) -> Option<GraphIntent> {
    let (graph_ctx, geometry) = (ctx.graph_ctx(), ctx.geometry());
    let port = start.input_port()?;
    let pointer = ui.pointer_pos()?;
    if !geometry.node_screen_rect(port.node_id)?.contains(pointer) {
        return None;
    }
    // Don't overwrite an existing const value.
    let input = graph_ctx.node(port.node_id)?.input(port.port)?;
    if matches!(input.binding(), Some(Binding::Const(_))) {
        return None;
    }
    let default = input.default()?;
    Some(GraphIntent::set_input(port, Binding::Const(default)))
}

/// Whether the pointer is over the canvas but not over any node body, the "released into empty
/// space" condition that offers the new-node palette. Both halves are screen-space, as the raw
/// pointer is; the node half comes off `CanvasGeometry`'s snapshot of this frame's body rects,
/// taken in the same sweep `const_drop` reads.
fn dropped_on_empty_canvas(ui: &mut Ui, graph: GraphId, geometry: &CanvasGeometry) -> bool {
    let Some(pointer) = ui.pointer_pos() else {
        return false;
    };
    let over_canvas = ui
        .response_for(canvas::outer_wid(graph))
        .rect
        .is_some_and(|r| r.contains(pointer));
    over_canvas && !geometry.over_any_node(pointer)
}

/// The `GraphIntent::SetInput` binding a snapped `(start, end)` `PortRef` pair (one `Input`,
/// one `Output`, guaranteed by [`scan_snap_target`]) commits. A cycle-forming pair never
/// reaches here: [`scan_snap_target`] refuses to snap one, and `GraphIntent::into_step`
/// rejects any that slips through.
fn wire(start: PortRef, end: PortRef) -> GraphIntent {
    let pair = start
        .input_port()
        .zip(end.output_port())
        .or_else(|| end.input_port().zip(start.output_port()));
    // Both entries into a commit snap through `scan_snap_target`, which only offers
    // `start.kind.opposite()`. A same-kind pair means that invariant broke upstream, and
    // silently dropping it would hide the break behind a wire that just doesn't land.
    let Some((input, output)) = pair else {
        unreachable!("a wire committed a {:?} → {:?} pair", start.kind, end.kind);
    };
    GraphIntent::SetInput {
        input,
        to: Some(Binding::Wire(output)),
    }
}

#[cfg(test)]
mod tests;
