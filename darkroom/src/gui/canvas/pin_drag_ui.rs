//! Cmd+LMB-drag-from-output-port gesture that creates a pinned output. A
//! sibling of [`crate::gui::canvas::connection_ui::ConnectionUI`], but with
//! no compatible target to snap to: releasing anywhere pins the port, so the
//! drag exists to give the gesture a deliberate motion rather than to aim at
//! a destination — a plain click stays free for the port's other chords.

use aperture::Ui;
use glam::Vec2;

use crate::core::document::{PortKind, PortRef};
use crate::core::edit::intent::Intent;
use crate::gui::app::AppContext;
use crate::gui::canvas::connection_ui::port_data_type;
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::canvas::{node_ports, pointer_world};
use crate::gui::node::port_color::port_color;
use crate::gui::node::port_row::pin_drag_preview;
use crate::gui::node::set_output_pinned;
use crate::gui::scene::Scene;

/// The output port a Cmd+drag latched onto, if any. Identity-only — the
/// port's center resolves every frame from `CanvasGeometry`, matching
/// `ConnectionUI`'s in-flight wire.
#[derive(Default, Debug)]
pub(crate) struct PinDragUi {
    start: Option<PortRef>,
}

impl PinDragUi {
    /// Whether a pin-creation drag is in flight — feeds the shared wire-fade
    /// tier alongside `ConnectionUI`/`SubscriptionUI`.
    pub(crate) fn dragging(&self) -> bool {
        self.start.is_some()
    }

    /// Latch a fresh Cmd+drag from an output port, then resolve on release:
    /// unlike a connection or subscription wire there's no compatible target
    /// to snap to, so releasing *anywhere* pins the port. Esc cancels
    /// without pinning.
    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        geometry: &CanvasGeometry,
        out: &mut Vec<Intent>,
    ) {
        // Latch a fresh drag only when idle.
        if self.start.is_none()
            && ui.modifiers().ctrl
            && let Some(start) = scan_drag_start(geometry, scene)
        {
            self.start = Some(start);
        }
        if ui.escape_pressed() {
            self.start = None;
            return;
        }
        let Some(start) = self.start else {
            return;
        };
        // `CanvasGeometry::dragging` rolls up `drag_delta().is_some() ||
        // drag_started()`; its transition to `false` is the release edge.
        if geometry.ports.dragging(start) {
            return;
        }
        out.push(set_output_pinned(start, true));
        self.start = None;
    }

    /// Paint the in-flight preview: the pin's bezier+satellite shape from
    /// the source port to the live cursor, tinted by the port's own
    /// data-type color — matching `ConnectionUI::draw_in_flight`.
    pub(crate) fn draw_in_flight(
        &self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        geometry: &CanvasGeometry,
        canvas_origin: Vec2,
    ) {
        let Some(start) = self.start else { return };
        let Some(port_center) = geometry.ports.center(start) else {
            return;
        };
        let Some(cursor) = pointer_world(ui, scene, canvas_origin) else {
            return;
        };
        let ty = port_data_type(scene, start).unwrap_or_default();
        let color = port_color(ctx.theme, &ty, PortKind::Output, false);
        pin_drag_preview(
            ui,
            ctx.theme,
            ctx.theme.port_size * 0.5,
            port_center,
            cursor,
            color,
        );
    }
}

/// First output port whose drag started this frame, or `None`.
fn scan_drag_start(geometry: &CanvasGeometry, scene: &Scene) -> Option<PortRef> {
    for n in &scene.nodes {
        for port in node_ports(n, PortKind::Output) {
            if geometry.ports.drag_started(port) {
                return Some(port);
            }
        }
    }
    None
}
