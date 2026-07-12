//! A pinned output's satellite: the bezier + marker showing a port's value is
//! pushed live to the GUI. A sibling of [`crate::gui::canvas::connection_ui::ConnectionUI`],
//! drawn at the canvas level (like a wire) rather than nested in the node
//! body, since a dragged satellite can end up anywhere on the canvas — not
//! just overhanging its own node.
//!
//! Owns two gestures, unified into one drag state since they differ only in
//! how they *start*: Cmd+drag from an output port's circle creates a fresh
//! pin; a plain drag on an existing pin's satellite repositions it. Both
//! resolve identically on release (see [`PinUi::apply`]).

use aperture::{Brush, Color, Configure, Panel, Rect, Sense, Sizing, Ui, WidgetId};
use glam::Vec2;
use scenarium::graph::OutputPort;

use crate::core::document::{PortKind, PortRef};
use crate::core::edit::intent::Intent;
use crate::gui::app::AppContext;
use crate::gui::canvas::breaker::BreakerProbe;
use crate::gui::canvas::connection_ui::port_data_type;
use crate::gui::canvas::cull::wire_visible;
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::canvas::wire::{CubicHandles, WireEmphasis, add_cubic_wire, cubic_handles};
use crate::gui::canvas::{inner_canvas_widget_id, node_ports, pointer_world};
use crate::gui::node::port_color::port_color;
use crate::gui::node::set_output_pinned;
use crate::gui::scene::{Scene, SceneOutput};
use crate::gui::theme::Theme;
use crate::gui::widgets::support::dot;

/// Radius of a pinned output's satellite circle, as a multiple of the
/// port's own radius.
const PIN_SATELLITE_SCALE: f32 = 1.4;

/// Rightward offset from the port circle's edge to the satellite circle's
/// center, before the satellite's own radius.
const PIN_REACH: f32 = 10.0;

/// How far above the port's own center the satellite circle sits.
const PIN_RISE: f32 = 12.0;

/// A pinned output's bezier + satellite geometry, anchored at `port_center`.
/// Pure so both the paint and the breaker hit-test derive the identical
/// shape from the same numbers.
struct PinGeometry {
    p0: Vec2,
    p1: Vec2,
    p2: Vec2,
    p3: Vec2,
    satellite_center: Vec2,
    satellite_radius: f32,
}

/// Bezier + satellite shape between `port_center` and `satellite_center` —
/// shared by a committed pin's resolved offset ([`pin_geometry`]) and a live
/// drag's cursor-following preview ([`PinUi::draw_in_flight`]). The control
/// handles are [`cubic_handles`], the same forward/backward-reach shape the
/// data wire uses (a pin's port and satellite are just another
/// output-ish-anchor-to-far-end pair) — so a pin dragged in any direction
/// bows exactly like a wire pulled the same way.
fn pin_curve(port_center: Vec2, satellite_center: Vec2, satellite_radius: f32) -> PinGeometry {
    let handles = cubic_handles(port_center, satellite_center);
    PinGeometry {
        p0: port_center,
        p1: handles.p1,
        p2: handles.p2,
        p3: satellite_center,
        satellite_center,
        satellite_radius,
    }
}

/// A committed pin's geometry, given its already-resolved `offset` (a
/// custom drag position, or [`default_pin_offset`]).
fn pin_geometry(port_center: Vec2, radius: f32, offset: Vec2) -> PinGeometry {
    pin_curve(
        port_center,
        port_center + offset,
        radius * PIN_SATELLITE_SCALE,
    )
}

/// The satellite offset a pin with no stored custom position falls back to:
/// up and to the right of the port circle.
fn default_pin_offset(radius: f32) -> Vec2 {
    Vec2::new(radius + PIN_REACH + radius * PIN_SATELLITE_SCALE, -PIN_RISE)
}

/// `output`'s satellite offset: its stored custom position
/// ([`crate::core::document::GraphView::pin_offsets`], mirrored onto
/// [`SceneOutput::pin_offset`]) if it's ever been dragged, else
/// [`default_pin_offset`].
fn resolved_pin_offset(output: &SceneOutput, radius: f32) -> Vec2 {
    output
        .pin_offset
        .unwrap_or_else(|| default_pin_offset(radius))
}

/// Paint one pin's bezier in `color`, shared by a committed pin
/// ([`PinUi::draw`]) and the in-flight drag preview
/// ([`PinUi::draw_in_flight`]). Draws through [`add_cubic_wire`], the same
/// primitive every data/event/subscription wire uses, so a pin reads as one
/// more wire rather than a bespoke shape.
fn paint_pin_bezier(ui: &mut Ui, theme: &Theme, g: &PinGeometry, color: Color) {
    add_cubic_wire(
        ui,
        g.p0,
        g.p3,
        CubicHandles { p1: g.p1, p2: g.p2 },
        theme.connection_width,
        Brush::Solid(color),
    );
}

/// Record the satellite's drag-sensing widget at `g`'s position and paint
/// its dot inside — shared by a committed pin ([`PinUi::draw`]) and the
/// in-flight drag preview ([`PinUi::draw_in_flight`]), so the *same* widget
/// id keeps recording every frame regardless of which one paints it.
/// Critical while a drag is live: if the widget stopped being recorded
/// mid-drag, `CanvasGeometry::rebuild` would poll a stale, empty response
/// next frame and read the drag as released — which is exactly the "drag
/// stops after one frame" bug this fixes. `draw`'s committed pass skips the
/// currently-dragged port's bezier/color resolution, but the widget itself
/// must keep recording continuously across both passes.
fn record_satellite(ui: &mut Ui, port: OutputPort, g: &PinGeometry, color: Color) {
    let d = g.satellite_radius * 2.0;
    Panel::zstack()
        .id(pin_satellite_wid(port))
        .position(g.satellite_center - Vec2::splat(g.satellite_radius))
        .size((Sizing::Fixed(d), Sizing::Fixed(d)))
        .sense(Sense::DRAG)
        .show(ui, |ui| {
            dot(
                ui,
                g.satellite_radius,
                g.satellite_radius,
                g.satellite_radius,
                color,
            );
        });
}

/// True if the active breaker gesture crosses `g`'s pin glyph — either the
/// connecting bezier or the satellite circle's bounding box (matching how a
/// node body's breaker hit-test uses its rect rather than an exact shape).
fn pin_targeted(probe: &BreakerProbe<'_>, g: &PinGeometry) -> bool {
    if probe.crosses_cubic(g.p0, g.p1, g.p2, g.p3) {
        return true;
    }
    let d = g.satellite_radius * 2.0;
    let satellite_rect = Rect::new(
        g.satellite_center.x - g.satellite_radius,
        g.satellite_center.y - g.satellite_radius,
        d,
        d,
    );
    probe.crosses_rect(satellite_rect)
}

/// Stable widget id for a pinned output's satellite marker — the drag
/// target for repositioning it. Reconstructible from the port so
/// [`CanvasGeometry::rebuild`] can poll its response without a cache.
pub(crate) fn pin_satellite_wid(port: OutputPort) -> WidgetId {
    WidgetId::from_hash(("graph.node.pin_satellite", port.node_id, port.port_idx))
}

/// The output port a drag latched onto, if any — either a Cmd+drag off its
/// circle (creating a fresh pin) or a plain drag off its existing satellite
/// (repositioning one). Identity-only — the port's center resolves every
/// frame from `CanvasGeometry`, and the satellite's drag state is polled
/// directly (see `satellite_dragging`), since only one pin drag is ever in
/// flight at once.
#[derive(Default, Debug)]
pub(crate) struct PinUi {
    start: Option<PortRef>,
}

impl PinUi {
    /// Whether a pin drag (either kind) is in flight — feeds the shared
    /// wire-fade tier alongside `ConnectionUI`/`SubscriptionUI`.
    pub(crate) fn dragging(&self) -> bool {
        self.start.is_some()
    }

    /// Latch a fresh drag — Cmd+drag from an output port's circle, or a
    /// plain drag from an existing pin's satellite — then resolve on
    /// release. Unlike a connection or subscription wire there's no
    /// compatible target to snap to, so releasing *anywhere* commits at the
    /// cursor's final position. Esc cancels without committing.
    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        geometry: &CanvasGeometry,
        out: &mut Vec<Intent>,
    ) {
        if self.start.is_none() {
            if ui.modifiers().ctrl
                && let Some(port) = scan_port_drag_start(geometry, scene)
            {
                self.start = Some(port);
            } else if let Some(port) = scan_satellite_drag_start(ui, scene) {
                self.start = Some(port);
            }
        }
        if ui.escape_pressed() {
            self.start = None;
            return;
        }
        let Some(port) = self.start else {
            return;
        };
        let out_port = OutputPort::new(port.node_id, port.port_idx);
        // Whichever widget the drag actually latched on (the port circle
        // for a fresh pin, the satellite for a reposition) reports the
        // live drag; the other stays permanently idle, so this OR
        // correctly detects the release edge either way. Only one pin
        // drag is ever in flight, so this polls the satellite response
        // directly rather than caching drag state for every pinned port.
        if geometry.ports.dragging(port) || satellite_dragging(ui, out_port) {
            return;
        }
        let canvas_origin = ui
            .response_for(inner_canvas_widget_id())
            .layout_rect
            .map(|r| r.min)
            .unwrap_or(Vec2::ZERO);
        if let (Some(port_center), Some(cursor)) = (
            geometry.ports.center(port),
            pointer_world(ui, scene, canvas_origin),
        ) {
            // Always push both: on a reposition the port is already
            // pinned, so `SetOutputPinned{true}` builds to a no-op and the
            // undo layer drops it — correct for either gesture without
            // tracking which one latched.
            out.push(set_output_pinned(port, true));
            out.push(Intent::SetPinOffset {
                node_id: port.node_id,
                port_idx: port.port_idx,
                to: cursor - port_center,
            });
        }
        self.start = None;
    }

    /// Paint every pinned output's committed bezier + satellite, marking
    /// those the active breaker crosses as broken via
    /// `probe.mark_broken_pin` for the breaker's release-frame drain. The
    /// satellite itself is a small drag-sensing panel (positioned
    /// world-space, like the inspector panels) so it can be grabbed to
    /// reposition. The one currently being dragged is skipped —
    /// `draw_in_flight` paints its live preview instead.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw(
        &self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        geometry: &CanvasGeometry,
        visible: Option<Rect>,
        probe: &mut BreakerProbe<'_>,
        emphasis: &WireEmphasis,
    ) {
        let theme = ctx.theme;
        let radius = theme.port_size * 0.5;
        for n in &scene.nodes {
            for (i, output) in scene.outputs(n.outputs).iter().enumerate() {
                if !output.pinned {
                    continue;
                }
                let port_ref = PortRef {
                    node_id: n.id,
                    kind: PortKind::Output,
                    port_idx: i,
                };
                if self.start == Some(port_ref) {
                    continue;
                }
                let Some(port_center) = geometry.ports.center(port_ref) else {
                    continue;
                };
                let g = pin_geometry(port_center, radius, resolved_pin_offset(output, radius));
                let handles = CubicHandles { p1: g.p1, p2: g.p2 };
                if !wire_visible(visible, g.p0, &handles, g.p3) {
                    continue;
                }
                let out_port = OutputPort::new(n.id, i);
                let broken = pin_targeted(probe, &g);
                if broken {
                    probe.mark_broken_pin(out_port);
                }
                // Like a data wire, the bezier's emphasis follows *either*
                // endpoint's hover — the port circle or the satellite.
                let sat_hover = satellite_hovered(ui, out_port);
                let hovered =
                    !broken && emphasis.hovered(geometry.ports.is_hovered(port_ref) || sat_hover);
                let base = port_color(theme, &output.ty, PortKind::Output, false);
                let wire_color = if broken {
                    theme.colors.connection_broken
                } else {
                    emphasis.tint(base, hovered)
                };
                let width = emphasis.width(theme.connection_width, hovered || broken);
                add_cubic_wire(ui, g.p0, g.p3, handles, width, Brush::Solid(wire_color));
                // Unlike the bezier, the satellite's own fill brightens on
                // its *own* direct hover, like a port circle — not the
                // wire-style endpoint emphasis.
                let satellite_fill = if broken {
                    theme.colors.connection_broken
                } else {
                    port_color(theme, &output.ty, PortKind::Output, sat_hover)
                };
                record_satellite(ui, out_port, &g, satellite_fill);
            }
        }
    }

    /// Paint the in-flight preview: the pin's bezier+satellite shape from
    /// the source port to the live cursor, tinted by the port's own
    /// data-type color — matching `ConnectionUI::draw_in_flight`. Covers
    /// both gestures (create and reposition): both fix the port end and
    /// follow the cursor with the satellite end.
    pub(crate) fn draw_in_flight(
        &self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        geometry: &CanvasGeometry,
        canvas_origin: Vec2,
    ) {
        let Some(port) = self.start else { return };
        let Some(port_center) = geometry.ports.center(port) else {
            return;
        };
        let Some(cursor) = pointer_world(ui, scene, canvas_origin) else {
            return;
        };
        let ty = port_data_type(scene, port).unwrap_or_default();
        let color = port_color(ctx.theme, &ty, PortKind::Output, false);
        let radius = ctx.theme.port_size * 0.5;
        let g = pin_curve(port_center, cursor, radius * PIN_SATELLITE_SCALE);
        paint_pin_bezier(ui, ctx.theme, &g, color);
        // Keep recording the satellite widget at its live position every
        // frame the drag is held — see `record_satellite`'s doc for why
        // this can't be skipped.
        record_satellite(ui, OutputPort::new(port.node_id, port.port_idx), &g, color);
    }
}

/// First output port whose circle's drag started this frame, or `None`.
fn scan_port_drag_start(geometry: &CanvasGeometry, scene: &Scene) -> Option<PortRef> {
    for n in &scene.nodes {
        for port in node_ports(n, PortKind::Output) {
            if geometry.ports.drag_started(port) {
                return Some(port);
            }
        }
    }
    None
}

/// First pinned output whose satellite's drag started this frame, or
/// `None`. Only a pinned output has a satellite widget at all.
fn scan_satellite_drag_start(ui: &Ui, scene: &Scene) -> Option<PortRef> {
    for n in &scene.nodes {
        for (i, output) in scene.outputs(n.outputs).iter().enumerate() {
            if !output.pinned {
                continue;
            }
            let port = OutputPort::new(n.id, i);
            if ui.response_for(pin_satellite_wid(port)).drag_started() {
                return Some(PortRef {
                    node_id: n.id,
                    kind: PortKind::Output,
                    port_idx: i,
                });
            }
        }
    }
    None
}

/// `true` while a drag started on `port`'s satellite is still live. Only
/// one pin drag is ever in flight (unlike ports/events/subs, which can
/// each have many simultaneously-relevant widgets), so this polls the
/// widget's response directly rather than through a `CanvasGeometry`
/// domain caching state for every pinned port.
fn satellite_dragging(ui: &Ui, port: OutputPort) -> bool {
    let r = ui.response_for(pin_satellite_wid(port));
    r.drag_started() || r.drag_delta().is_some()
}

/// `true` when the pointer is directly over `port`'s satellite this frame —
/// polled the same way as [`satellite_dragging`], for the same reason (only
/// one pin is ever relevant at a time, no `CanvasGeometry` domain needed).
/// Drives the bezier's endpoint-hover emphasis (like a data wire's) and the
/// satellite's own direct-hover fill (like a port circle's).
fn satellite_hovered(ui: &Ui, port: OutputPort) -> bool {
    ui.response_for(pin_satellite_wid(port)).hovered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::canvas::breaker::{BreakerState, cubic_point};
    use aperture::PointerButton;

    /// A `BreakerProbe` wrapping `state`, at the origin — every test here
    /// works in a local frame with no canvas offset to convert.
    fn probe_for(state: &mut BreakerState) -> BreakerProbe<'_> {
        BreakerProbe {
            origin: Vec2::ZERO,
            state: Some(state),
        }
    }

    #[test]
    fn pin_targeted_hits_the_satellite_circle_but_not_empty_space() {
        let port_center = Vec2::ZERO;
        let radius = 5.0;
        let g = pin_geometry(port_center, radius, default_pin_offset(radius));

        let mut hit = BreakerState::start(g.satellite_center, PointerButton::Right);
        assert!(
            pin_targeted(&probe_for(&mut hit), &g),
            "a breaker sample landing dead-center in the satellite must register"
        );

        let mut miss = BreakerState::start(Vec2::new(1000.0, 1000.0), PointerButton::Right);
        assert!(
            !pin_targeted(&probe_for(&mut miss), &g),
            "a breaker far from the glyph must not register"
        );
    }

    #[test]
    fn pin_targeted_hits_the_connecting_bezier() {
        let port_center = Vec2::ZERO;
        let radius = 5.0;
        let g = pin_geometry(port_center, radius, default_pin_offset(radius));
        // `t = 0.53`, not `0.5`: `intersects_cubic` samples the curve at 16
        // evenly-spaced points, and `t = 0.5` lands exactly on one of them —
        // a vertical probe through that exact vertex is the degenerate
        // "touch, don't cross" case the strict crossing test intentionally
        // rejects (see `intersects_cubic_diagonal_through_straight_wire`).
        let mid = cubic_point(g.p0, g.p1, g.p2, g.p3, 0.53);

        // A long vertical scribble through that point, clear of the
        // satellite circle, so this exercises the bezier crossing — not the
        // satellite rect — path.
        let mut state = BreakerState::start(mid + Vec2::new(0.0, -50.0), PointerButton::Right);
        state.add_point(mid + Vec2::new(0.0, 50.0));
        assert!(pin_targeted(&probe_for(&mut state), &g));
    }
}
