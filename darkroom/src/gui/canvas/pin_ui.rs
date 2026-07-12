//! A pinned output's preview widget: a small card floating above the node
//! bodies, connected back to its port by a bezier wire. A sibling of
//! [`crate::gui::canvas::connection_ui::ConnectionUI`], drawn at the canvas
//! level (like a wire) rather than nested in the node body, since a dragged
//! widget can end up anywhere on the canvas — not just overhanging its own
//! node.
//!
//! Owns one gesture, unified across how it *starts*: Cmd+drag from an
//! output port's circle creates a fresh pin; a plain drag on an existing
//! preview widget repositions it. Both commit continuously — every frame
//! the drag is held, not just on release — exactly like a node body drag
//! (see [`crate::gui::node::NodeUI`]'s `DragAnchor`/`prepass`), coalesced by
//! `GestureKey::PinDrag` into one undo entry. Since the position lands in
//! `Document` (and `Scene` rebuilds) before the record pass, there's no
//! separate in-flight preview to paint — the widget's real position already
//! reflects the live drag by the time [`PinUi::draw`] runs.

use std::collections::HashMap;
use std::sync::Arc;

use aperture::{
    Background, Brush, Color, Configure, Corners, ImageFilter, ImageFit, ImageHandle, Panel, Rect,
    Sense, Shadow, Shape, Sizing, Spacing, Stroke, Text, TextWrap, Ui, WidgetId,
};
use glam::Vec2;
use scenarium::data::{CustomValue, DataType, DynamicValue};
use scenarium::graph::OutputPort;

use crate::core::document::{PortKind, PortRef};
use crate::core::edit::intent::Intent;
use crate::gui::app::AppContext;
use crate::gui::canvas::breaker::BreakerProbe;
use crate::gui::canvas::cull::wire_visible;
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::canvas::node_ports;
use crate::gui::canvas::wire::{CubicHandles, WireEmphasis, add_cubic_wire, cubic_handles};
use crate::gui::image_viewer::convert_image_value;
use crate::gui::node::port_color::port_color;
use crate::gui::node::port_row::port_circle_wid;
use crate::gui::node::set_output_pinned;
use crate::gui::scene::{Scene, SceneOutput};
use crate::gui::theme::Theme;
use crate::gui::widgets::support::sized_text;

/// Fixed footprint of a pinned output's preview widget, canvas-world units —
/// a stable size regardless of content, so a drag never has to re-measure
/// and an image never grows the widget. An image letterboxes inside via
/// `Contain`; a non-image value's formatted text centers in the same frame.
const PREVIEW_WIDTH: f32 = 120.0;
const PREVIEW_HEIGHT: f32 = 90.0;

/// Longest side, in pixels, an image-typed pinned value is downscaled to
/// before upload — small enough to stay cheap with many simultaneous pins.
const PREVIEW_TEXTURE_DIM: u32 = 256;

/// Gap between the port circle's edge and the preview widget's near edge,
/// at the default (never-dragged) position.
const PIN_GAP: f32 = 16.0;

/// The default widget-center offset (from the port center) a freshly
/// pinned output falls back to before it's ever been dragged: up and to
/// the right of the port, clear of the port circle.
fn default_pin_offset() -> Vec2 {
    Vec2::new(
        PREVIEW_WIDTH * 0.5 + PIN_GAP,
        -(PREVIEW_HEIGHT * 0.5 + PIN_GAP),
    )
}

/// `output`'s widget-center offset from its port: its stored custom
/// position ([`crate::core::document::GraphView::pin_offsets`], mirrored
/// onto [`SceneOutput::pin_offset`]) if it's ever been dragged, else
/// [`default_pin_offset`].
fn resolved_pin_offset(output: &SceneOutput) -> Vec2 {
    output.pin_offset.unwrap_or_else(default_pin_offset)
}

/// Whether `ty` is an image value — the pinned output's preview widget
/// shows a thumbnail for these, and its formatted text for everything else.
fn is_image_type(ty: &DataType) -> bool {
    matches!(ty, DataType::Custom(id) if *id == *lens::IMAGE_TYPE_ID)
}

/// The port or widget a drag latched onto, and where its offset started —
/// mirrors `NodeUI`'s `DragAnchor`: every later frame's committed offset is
/// `start_offset + drag_delta`, not a running integration over the moving
/// widget.
#[derive(Clone, Copy, Debug)]
struct PinDragAnchor {
    port: OutputPort,
    /// Zero for a freshly created pin (it tracks cursor-minus-port from the
    /// press); the widget's already-resolved offset for a reposition (so it
    /// continues from where it visually sits instead of jumping).
    start_offset: Vec2,
    /// Captured at latch so later frames can `ui.response_for(widget_id)`
    /// directly: the port circle for a fresh pin, the preview widget for a
    /// reposition.
    widget_id: WidgetId,
}

/// An uploaded preview texture, kept alive across frames (an `ImageHandle`
/// frees its GPU texture when its last clone drops) and reconverted only
/// when the pinned value it came from actually changed.
struct CachedPreview {
    /// Identity of the pinned value this texture was converted from — an
    /// `Arc::ptr_eq` hit against a fresh push skips a redundant
    /// reconvert+reupload.
    source: Arc<dyn CustomValue>,
    handle: ImageHandle,
}

impl std::fmt::Debug for CachedPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedPreview")
            .field("source_type", &self.source.type_id())
            .field("handle", &self.handle)
            .finish()
    }
}

/// Pinned outputs' preview-widget drag state plus their uploaded thumbnail
/// cache. Only one pin drag is ever in flight, so `drag` is a single slot;
/// `previews` is keyed by port since many pins can hold thumbnails at once.
#[derive(Default, Debug)]
pub(crate) struct PinUi {
    drag: Option<PinDragAnchor>,
    previews: HashMap<OutputPort, CachedPreview>,
}

impl PinUi {
    /// Whether a pin drag is in flight — feeds the shared wire-fade tier
    /// alongside `ConnectionUI`/`SubscriptionUI`.
    pub(crate) fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Continuous per-frame drag, exactly like a node body drag: latch on a
    /// Cmd+drag off an output port's circle (creates a fresh pin) or a
    /// plain drag off an existing preview widget (repositions it), then
    /// push `Intent::SetPinOffset` every frame the drag is held.
    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        geometry: &CanvasGeometry,
        out: &mut Vec<Intent>,
    ) {
        if let Some(anchor) = self.drag {
            if scene.nodes.by_key(&anchor.port.node_id).is_none() {
                // Stale: the node vanished mid-drag (breaker/undo). Drop
                // rather than build an intent against a missing node.
                self.drag = None;
            } else {
                let resp = ui.response_for(anchor.widget_id);
                if resp.drag_started() {
                    // A fresh gesture just replaced this one on the same
                    // widget; drop rather than fire with stale start data.
                    self.drag = None;
                } else if let Some(delta) = resp.drag_delta() {
                    let to = anchor.start_offset + delta / scene.viewport.safe_zoom();
                    out.push(Intent::SetPinOffset {
                        node_id: anchor.port.node_id,
                        port_idx: anchor.port.port_idx,
                        to,
                    });
                    return;
                } else {
                    // No delta means the drag isn't latched anymore
                    // (release or pointer-left-surface).
                    self.drag = None;
                }
            }
        }
        if ui.modifiers().ctrl
            && let Some(port) = scan_port_drag_start(geometry, scene)
        {
            let widget_id = port_circle_wid(port);
            out.push(set_output_pinned(port, true));
            self.drag = Some(PinDragAnchor {
                port: OutputPort::new(port.node_id, port.port_idx),
                start_offset: Vec2::ZERO,
                widget_id,
            });
        } else if let Some((port, start_offset)) = scan_widget_drag_start(ui, scene) {
            self.drag = Some(PinDragAnchor {
                port,
                start_offset,
                widget_id: pin_preview_wid(port),
            });
        }
    }

    /// Paint every pinned output's bezier + preview widget on top of the
    /// node bodies, marking those the active breaker crosses as broken via
    /// `probe.mark_broken_pin` for the breaker's release-frame drain.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        geometry: &CanvasGeometry,
        visible: Option<Rect>,
        probe: &mut BreakerProbe<'_>,
        emphasis: &WireEmphasis,
    ) {
        self.prune_previews(scene);
        let theme = ctx.theme;
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
                let Some(port_center) = geometry.ports.center(port_ref) else {
                    continue;
                };
                let out_port = OutputPort::new(n.id, i);
                let box_center = port_center + resolved_pin_offset(output);
                let handles = cubic_handles(port_center, box_center);
                if !wire_visible(visible, port_center, &handles, box_center) {
                    continue;
                }
                let box_rect = Rect::new(
                    box_center.x - PREVIEW_WIDTH * 0.5,
                    box_center.y - PREVIEW_HEIGHT * 0.5,
                    PREVIEW_WIDTH,
                    PREVIEW_HEIGHT,
                );
                let broken = pin_targeted(probe, port_center, &handles, box_center, box_rect);
                if broken {
                    probe.mark_broken_pin(out_port);
                }
                let box_hover = preview_hovered(ui, out_port);
                let hovered =
                    !broken && emphasis.hovered(geometry.ports.is_hovered(port_ref) || box_hover);
                let base = port_color(theme, &output.ty, PortKind::Output, false);
                let wire_color = if broken {
                    theme.colors.connection_broken
                } else {
                    emphasis.tint(base, hovered)
                };
                let width = emphasis.width(theme.connection_width, hovered || broken);
                add_cubic_wire(
                    ui,
                    port_center,
                    box_center,
                    handles,
                    width,
                    Brush::Solid(wire_color),
                );

                // Unlike the wire, the widget's own border brightens on its
                // *own* direct hover, like a port circle — not the
                // wire-style endpoint emphasis.
                let border = if broken {
                    theme.colors.connection_broken
                } else {
                    port_color(theme, &output.ty, PortKind::Output, box_hover)
                };
                let value = ctx.run_state.pinned_value(n.id, i);
                let texture = if is_image_type(&output.ty) {
                    value.and_then(|v| self.resolve_preview(ui, out_port, v))
                } else {
                    None
                };
                let text = texture.is_none().then(|| {
                    value
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "not yet run".to_owned())
                });
                draw_preview_box(
                    ui,
                    theme,
                    out_port,
                    box_center,
                    border,
                    texture.as_ref(),
                    text.as_deref(),
                );
            }
        }
    }

    /// The current thumbnail texture for `port`'s pinned image value,
    /// converting + uploading fresh only when the value changed since last
    /// time (an `Arc` identity miss) or nothing's cached yet. `None` when
    /// `value` isn't a (decodable) image — the caller falls back to text.
    fn resolve_preview(
        &mut self,
        ui: &Ui,
        port: OutputPort,
        value: &DynamicValue,
    ) -> Option<ImageHandle> {
        let DynamicValue::Custom(data) = value else {
            self.previews.remove(&port);
            return None;
        };
        if let Some(cached) = self.previews.get(&port)
            && Arc::ptr_eq(&cached.source, data)
        {
            return Some(cached.handle.clone());
        }
        let (image, _, _) = convert_image_value(value, PREVIEW_TEXTURE_DIM).ok()?;
        let handle = ui.register_image(image);
        self.previews.insert(
            port,
            CachedPreview {
                source: Arc::clone(data),
                handle: handle.clone(),
            },
        );
        Some(handle)
    }

    /// Drop cached textures for outputs no longer pinned (or removed) —
    /// otherwise a session that pins/unpins/deletes many nodes over time
    /// would leak textures indefinitely.
    fn prune_previews(&mut self, scene: &Scene) {
        self.previews.retain(|port, _| {
            scene.nodes.by_key(&port.node_id).is_some_and(|n| {
                scene
                    .outputs(n.outputs)
                    .get(port.port_idx)
                    .is_some_and(|o| o.pinned)
            })
        });
    }
}

/// Paint one pinned output's preview widget: a fixed-size card, letterboxed
/// image if `texture` is `Some`, else centered `text`. Senses `DRAG` so it
/// doubles as the reposition drag's grab target ([`pin_preview_wid`]).
fn draw_preview_box(
    ui: &mut Ui,
    theme: &Theme,
    port: OutputPort,
    center: Vec2,
    border: Color,
    texture: Option<&ImageHandle>,
    text: Option<&str>,
) {
    Panel::vstack()
        .id(pin_preview_wid(port))
        .position(center - Vec2::new(PREVIEW_WIDTH * 0.5, PREVIEW_HEIGHT * 0.5))
        .size((Sizing::Fixed(PREVIEW_WIDTH), Sizing::Fixed(PREVIEW_HEIGHT)))
        .sense(Sense::DRAG)
        .background(
            Background::rounded(
                theme.colors.node_fill,
                Corners::all(theme.node_corner_radius),
            )
            .with_stroke(Stroke::solid(border, 1.0))
            .with_shadow(Shadow::drop(
                theme.colors.node_ambient_shadow,
                Vec2::new(0.0, 3.0),
                8.0,
            )),
        )
        .show(ui, |ui| {
            if let Some(handle) = texture {
                ui.add_shape(
                    Shape::image(handle.clone())
                        .fit(ImageFit::Contain)
                        .filter(ImageFilter::Linear),
                );
                // Rounds the image's square corners into the card's own
                // fill, matching the card's own rounding — a plain image
                // fill ignores the panel's rounded background.
                ui.add_shape(Shape::WindowedRect {
                    local_rect: None,
                    corners: Corners::all(theme.node_corner_radius),
                    fill: theme.colors.node_fill.into(),
                    stroke: Stroke::solid(border, 1.0),
                });
            } else if let Some(text) = text {
                Panel::vstack()
                    .id_salt(("graph.node.pin_preview_text", port))
                    .size((Sizing::FILL, Sizing::FILL))
                    .padding(Spacing::all(8.0))
                    .show(ui, |ui| {
                        Text::new(text.to_owned())
                            .style(sized_text(ui, 11.0))
                            .text_wrap(TextWrap::Wrap)
                            .show(ui);
                    });
            }
        });
}

/// True if the active breaker gesture crosses the pin's glyph — either the
/// connecting bezier or the preview widget's rect (matching how a node
/// body's breaker hit-test uses its rect rather than an exact shape).
fn pin_targeted(
    probe: &BreakerProbe<'_>,
    port_center: Vec2,
    handles: &CubicHandles,
    box_center: Vec2,
    box_rect: Rect,
) -> bool {
    if probe.crosses_cubic(port_center, handles.p1, handles.p2, box_center) {
        return true;
    }
    probe.crosses_rect(box_rect)
}

/// Stable widget id for a pinned output's preview widget — the drag target
/// for repositioning it. Reconstructible from the port so
/// [`CanvasGeometry::rebuild`]-style polling can read its response without
/// a cache.
pub(crate) fn pin_preview_wid(port: OutputPort) -> WidgetId {
    WidgetId::from_hash(("graph.node.pin_preview", port.node_id, port.port_idx))
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

/// First pinned output whose preview widget's drag started this frame, with
/// its currently-resolved offset (the drag's start point) — or `None`. Only
/// a pinned output has a preview widget at all.
fn scan_widget_drag_start(ui: &Ui, scene: &Scene) -> Option<(OutputPort, Vec2)> {
    for n in &scene.nodes {
        for (i, output) in scene.outputs(n.outputs).iter().enumerate() {
            if !output.pinned {
                continue;
            }
            let port = OutputPort::new(n.id, i);
            if ui.response_for(pin_preview_wid(port)).drag_started() {
                return Some((port, resolved_pin_offset(output)));
            }
        }
    }
    None
}

/// `true` when the pointer is directly over `port`'s preview widget this
/// frame. Drives the bezier's endpoint-hover emphasis (like a data wire's)
/// and the widget's own direct-hover border tint (like a port circle's).
fn preview_hovered(ui: &Ui, port: OutputPort) -> bool {
    ui.response_for(pin_preview_wid(port)).hovered
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

    fn box_rect_at(center: Vec2) -> Rect {
        Rect::new(
            center.x - PREVIEW_WIDTH * 0.5,
            center.y - PREVIEW_HEIGHT * 0.5,
            PREVIEW_WIDTH,
            PREVIEW_HEIGHT,
        )
    }

    #[test]
    fn pin_targeted_hits_the_preview_box_but_not_empty_space() {
        let port_center = Vec2::ZERO;
        let box_center = port_center + default_pin_offset();
        let handles = cubic_handles(port_center, box_center);
        let rect = box_rect_at(box_center);

        let mut hit = BreakerState::start(box_center, PointerButton::Right);
        assert!(
            pin_targeted(
                &probe_for(&mut hit),
                port_center,
                &handles,
                box_center,
                rect
            ),
            "a breaker sample landing dead-center in the preview widget must register"
        );

        let mut miss = BreakerState::start(Vec2::new(1000.0, 1000.0), PointerButton::Right);
        assert!(
            !pin_targeted(
                &probe_for(&mut miss),
                port_center,
                &handles,
                box_center,
                rect
            ),
            "a breaker far from the glyph must not register"
        );
    }

    #[test]
    fn pin_targeted_hits_the_connecting_bezier() {
        // Synthetic, well-separated points (rather than `default_pin_offset`)
        // so the bezier's midpoint is unambiguously clear of the box rect —
        // this test exercises the bezier-crossing path, not the box-rect one.
        let port_center = Vec2::ZERO;
        let box_center = Vec2::new(300.0, 0.0);
        let handles = cubic_handles(port_center, box_center);
        let rect = box_rect_at(box_center);
        let mid = cubic_point(port_center, handles.p1, handles.p2, box_center, 0.53);

        let mut state = BreakerState::start(mid + Vec2::new(0.0, -80.0), PointerButton::Right);
        state.add_point(mid + Vec2::new(0.0, 80.0));
        assert!(pin_targeted(
            &probe_for(&mut state),
            port_center,
            &handles,
            box_center,
            rect
        ));
    }
}
