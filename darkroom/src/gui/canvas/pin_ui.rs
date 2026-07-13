//! A pinned output's wire, port-circle glyph, and drag gesture. The
//! widget's own look (the card, its header, its texture cache) lives in the
//! sibling [`crate::gui::canvas::pin_preview`] — this file draws the bezier
//! from the port to the widget's top-left corner, the port-circle glyph
//! peeking from under that corner (the same corner-peek trick as
//! [`crate::gui::node::header::subscription_pin`]), and owns the drag
//! gesture that positions it. A sibling of
//! [`crate::gui::canvas::connection_ui::ConnectionUI`], drawn at the canvas
//! level (like a wire) rather than nested in the node body, since a dragged
//! widget can end up anywhere on the canvas — not just overhanging its own
//! node.
//!
//! Owns one gesture, unified across how it *starts*: Cmd+drag from an
//! output port's circle creates a fresh pin; a plain drag on an existing
//! preview widget repositions it (dragging one that's part of a
//! multi-selection moves the whole group — nodes and other pins alike —
//! together, via the same [`crate::core::edit::intent::Intent::MoveSelection`]
//! [`crate::gui::node::NodeUI`]'s node-body drag emits). Both commit
//! continuously — every frame the drag is held, not just on release —
//! exactly like a node body drag (see `NodeUI`'s `DragAnchor`/`prepass`),
//! coalesced by `GestureKey::SelectionDrag` into one undo entry. Since the
//! position lands in `Document` (and `Scene` rebuilds) before the record
//! pass, there's no separate in-flight preview to paint — the widget's
//! real position already reflects the live drag by the time
//! [`PinUi::draw_wire`]/[`PinUi::draw_widget`] run.

use std::collections::BTreeSet;

use aperture::{Brush, Rect, Ui, WidgetId};
use glam::Vec2;
use scenarium::graph::{NodeId, OutputPort};

use crate::core::document::{PortKind, PortRef, SelectionKey};
use crate::core::edit::intent::Intent;
use crate::gui::app::AppContext;
use crate::gui::canvas::breaker::BreakerProbe;
use crate::gui::canvas::cull::wire_visible;
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::canvas::node_ports;
use crate::gui::canvas::pin_preview::{
    self, PREVIEW_HEIGHT, PREVIEW_WIDTH, PreviewCache, is_image_type, pin_preview_wid,
    preview_title,
};
use crate::gui::canvas::wire::{CubicHandles, WireEmphasis, add_cubic_wire, cubic_handles};
use crate::gui::node::click_intents;
use crate::gui::node::port_color::port_color;
use crate::gui::node::port_row::port_circle_wid;
use crate::gui::node::set_output_pinned;
use crate::gui::scene::{Scene, SceneOutput};
use crate::gui::widgets::support::dot;

/// Gap between the port circle's edge and the preview widget's near edge,
/// at the default (never-dragged) position.
const PIN_GAP: f32 = 16.0;

/// The port-relative offset (top-left corner, from the port center) a
/// pinned output's widget is placed at when it needs a sensible starting
/// point with no drag to derive one from — the context-menu pin toggle
/// (`crate::gui::node::port_row`) seeds a fresh pin's position with this;
/// dragging to create one instead starts it exactly at the port (see
/// [`PinUi::apply`]) so it visually "grows out of" the circle.
pub(crate) fn default_pin_offset() -> Vec2 {
    Vec2::new(PIN_GAP, -(PREVIEW_HEIGHT + PIN_GAP))
}

/// World-space rect of a pinned output's preview widget — the same fixed
/// footprint at the same absolute position [`draw_widget`](PinUi::draw_widget)
/// paints at. Shared by the rubber-band sweep's hit-test and
/// [`resolve_pin_geometry`]'s breaker/hover geometry.
pub(crate) fn pin_preview_rect(output: &SceneOutput) -> Rect {
    let top_left = output.pin_position;
    Rect::new(top_left.x, top_left.y, PREVIEW_WIDTH, PREVIEW_HEIGHT)
}

/// The lone-member `Intent::MoveSelection` that seeds `port`'s position —
/// shared by the two places a pin can come into existence with no drag
/// already in flight to place it naturally: this file's Cmd+drag creation
/// (seeded at the port center) and `port_row`'s context-menu pin toggle
/// (seeded at `port_center + `[`default_pin_offset`]`()`).
pub(crate) fn seed_pin_position_intent(port: OutputPort, position: Vec2) -> Intent {
    Intent::MoveSelection {
        grabbed: SelectionKey::Pin(port),
        nodes: vec![],
        pins: vec![(port, position)],
    }
}

/// Result of [`selected_group_positions`]: every selected node's position
/// and every selected pinned-output preview's absolute position, for a
/// group drag.
pub(crate) struct SelectedGroup {
    pub(crate) nodes: Vec<(NodeId, Vec2)>,
    pub(crate) pins: Vec<(OutputPort, Vec2)>,
}

/// Resolve the current selection into a [`SelectedGroup`] for a group drag
/// latched by grabbing either kind of member — shared by
/// [`crate::gui::node::NodeUI`]'s node-body drag and this file's
/// pin-widget drag, so both produce the same
/// [`Intent::MoveSelection`] group regardless of which member's press
/// started it.
pub(crate) fn selected_group_positions(
    scene: &Scene,
    selected: &BTreeSet<SelectionKey>,
) -> SelectedGroup {
    let nodes = scene
        .nodes
        .iter()
        .filter(|n| selected.contains(&SelectionKey::Node(n.id)))
        .map(|n| (n.id, n.pos))
        .collect();
    let mut pins = Vec::new();
    for n in &scene.nodes {
        for (i, output) in scene.outputs(n.outputs).iter().enumerate() {
            if !output.pinned {
                continue;
            }
            let port = OutputPort::new(n.id, i);
            if !selected.contains(&SelectionKey::Pin(port)) {
                continue;
            }
            pins.push((port, output.pin_position));
        }
    }
    SelectedGroup { nodes, pins }
}

/// The pin (or brand-new pin) a drag latched onto, and every member moving
/// with it — mirrors `NodeUI`'s `DragAnchor`: every later frame's committed
/// position is `start + drag_delta`, not a running integration over the
/// moving widget.
#[derive(Clone, Debug)]
struct PinDragAnchor {
    /// The pin the pointer latched. Keys the `response_for` lookup and the
    /// drag gesture.
    port: OutputPort,
    /// Every node moving with this drag and its position at drag start:
    /// the selected nodes when the grabbed pin was already part of a
    /// multi-selection, else empty (a lone pin drag/creation carries no
    /// nodes).
    start_node_positions: Vec<(NodeId, Vec2)>,
    /// Every pin moving with this drag (including the grabbed one) and its
    /// absolute position at drag start: the whole selection's pins when
    /// the grabbed pin was already selected, else just the grabbed pin —
    /// the port center itself for a freshly created pin (so it "grows out
    /// of" the port circle as the user drags), or the widget's
    /// already-resolved position for a reposition (so it continues from
    /// where it visually sits instead of jumping).
    start_pin_positions: Vec<(OutputPort, Vec2)>,
    /// Captured at latch so later frames can `ui.response_for(widget_id)`
    /// directly: the port circle for a fresh pin, the preview widget for a
    /// reposition.
    widget_id: WidgetId,
}

/// Pinned outputs' preview-widget drag state plus their uploaded thumbnail
/// cache. Only one pin drag is ever in flight, so `drag` is a single slot.
#[derive(Default, Debug)]
pub(crate) struct PinUi {
    drag: Option<PinDragAnchor>,
    previews: PreviewCache,
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
    /// push one `Intent::MoveSelection` every frame the drag is held.
    /// Grabbing a pin that's already part of a multi-selection drags the
    /// whole group (nodes and pins alike) together, exactly like grabbing
    /// an already-selected node does.
    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        geometry: &CanvasGeometry,
        selected: &BTreeSet<SelectionKey>,
        out: &mut Vec<Intent>,
    ) {
        if let Some(anchor) = self.drag.clone() {
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
                    let offset = delta / scene.viewport.safe_zoom();
                    let nodes = anchor
                        .start_node_positions
                        .iter()
                        .map(|(id, start)| (*id, *start + offset))
                        .collect();
                    let pins = anchor
                        .start_pin_positions
                        .iter()
                        .map(|(port, start)| (*port, *start + offset))
                        .collect();
                    out.push(Intent::MoveSelection {
                        grabbed: SelectionKey::Pin(anchor.port),
                        nodes,
                        pins,
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
            && let Some(port_ref) = scan_port_drag_start(geometry, scene)
        {
            let widget_id = port_circle_wid(port_ref);
            out.push(set_output_pinned(port_ref, true));
            let port = OutputPort::new(port_ref.node_id, port_ref.port_idx);
            // A brand-new pin isn't part of any selection yet — it drags
            // alone, starting exactly at the port (so it visually "grows
            // out of" the circle) and tracking the cursor from there.
            // Seeded here — rather than left to `set_output_pinned`'s zero
            // default — so this very first frame doesn't flash the widget
            // at the canvas origin before the drag below places it.
            if let Some(port_center) = geometry.ports.center(port_ref) {
                out.push(seed_pin_position_intent(port, port_center));
                self.drag = Some(PinDragAnchor {
                    port,
                    start_node_positions: vec![],
                    start_pin_positions: vec![(port, port_center)],
                    widget_id,
                });
            }
        } else if let Some((port, start_position)) = scan_widget_drag_start(ui, scene) {
            // Grabbing a pin already in the selection drags the whole
            // group (nodes + pins) together; grabbing an unselected pin
            // repositions it alone, leaving the selection untouched.
            let SelectedGroup {
                nodes: start_node_positions,
                pins: start_pin_positions,
            } = if selected.contains(&SelectionKey::Pin(port)) {
                selected_group_positions(scene, selected)
            } else {
                SelectedGroup {
                    nodes: vec![],
                    pins: vec![(port, start_position)],
                }
            };
            self.drag = Some(PinDragAnchor {
                port,
                start_node_positions,
                start_pin_positions,
                widget_id: pin_preview_wid(port),
            });
        }
    }

    /// Paint every pinned output's connecting bezier — called alongside
    /// `ConnectionUI`/`SubscriptionUI`'s wire draws, *before* the node
    /// bodies, so a pin wire passing behind an unrelated node goes under it
    /// like any other wire, instead of drawing on top. Marks a
    /// breaker-crossed pin via `probe.mark_broken_pin` for the breaker's
    /// release-frame drain — the same combined bezier-or-widget-rect test
    /// [`draw_widget`](Self::draw_widget) runs, so both halves agree on
    /// `broken` within the same frame even though they paint at different
    /// points in the pass. Only the wire's z-order changes here; the
    /// port-circle glyph and the card itself still float above everything
    /// (see [`draw_widget`](Self::draw_widget)).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_wire(
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
        for n in &scene.nodes {
            for (i, output) in scene.outputs(n.outputs).iter().enumerate() {
                if !output.pinned {
                    continue;
                }
                let Some(g) = resolve_pin_geometry(ui, geometry, probe, visible, n.id, i, output)
                else {
                    continue;
                };
                let port_ref = PortRef {
                    node_id: n.id,
                    kind: PortKind::Output,
                    port_idx: i,
                };
                let hovered = !g.broken
                    && emphasis.hovered(geometry.ports.is_hovered(port_ref) || g.box_hover);
                let base = port_color(theme, &output.ty, PortKind::Output, false);
                let wire_color = if g.broken {
                    theme.colors.connection_broken
                } else {
                    emphasis.tint(base, hovered)
                };
                let width = emphasis.width(theme.connection_width, hovered || g.broken);
                add_cubic_wire(
                    ui,
                    g.port_center,
                    g.top_left,
                    g.handles,
                    width,
                    Brush::Solid(wire_color),
                );
            }
        }
    }

    /// Paint every pinned output's port-circle glyph + preview widget, on
    /// top of the node bodies — the widget floats above everything even
    /// though its wire now draws under node bodies like any other wire
    /// (see [`draw_wire`](Self::draw_wire)).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_widget(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        geometry: &CanvasGeometry,
        visible: Option<Rect>,
        probe: &mut BreakerProbe<'_>,
        selected: &BTreeSet<SelectionKey>,
        out: &mut Vec<Intent>,
    ) {
        self.previews.prune(|port| {
            scene.nodes.by_key(&port.node_id).is_some_and(|n| {
                scene
                    .outputs(n.outputs)
                    .get(port.port_idx)
                    .is_some_and(|o| o.pinned)
            })
        });
        let theme = ctx.theme;
        for n in &scene.nodes {
            for (i, output) in scene.outputs(n.outputs).iter().enumerate() {
                if !output.pinned {
                    continue;
                }
                let Some(g) = resolve_pin_geometry(ui, geometry, probe, visible, n.id, i, output)
                else {
                    continue;
                };
                // The data-type accent lives *only* on the port-circle
                // glyph (brightening on the widget's own hover, like a
                // real port circle) — the widget's own border stays
                // neutral (see `pin_preview::draw_widget`), so the accent
                // doesn't double up right next to itself.
                let accent = if g.broken {
                    theme.colors.connection_broken
                } else {
                    port_color(theme, &output.ty, PortKind::Output, g.box_hover)
                };
                dot(
                    ui,
                    g.top_left.x,
                    g.top_left.y,
                    theme.port_size * 0.5,
                    accent,
                );

                // Same border-width formula as a node body (always the
                // selection width, so selecting the card never resizes it —
                // see `gui::node::draw_one`), so "in the selection" reads as
                // one visual language across nodes and pin previews.
                let is_selected = selected.contains(&SelectionKey::Pin(g.out_port));
                let border_width = theme.node_border_width * 2.0;
                let card_border = if g.broken {
                    theme.colors.connection_broken
                } else if is_selected {
                    theme.colors.selection_rect
                } else {
                    theme.colors.node_border
                };
                let value = ctx.run_state.pinned_value(n.id, i);
                let image = if is_image_type(&output.ty) {
                    value.and_then(|v| self.previews.resolve(ui, g.out_port, v))
                } else {
                    None
                };
                let text = image.is_none().then(|| {
                    value
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "not yet run".to_owned())
                });
                let response = pin_preview::draw_widget(
                    ui,
                    theme,
                    g.out_port,
                    g.top_left,
                    &preview_title(n.name.as_str(), output.name.as_str()),
                    card_border,
                    border_width,
                    image.as_ref(),
                    text.as_deref(),
                );
                // Click without drag → select, exactly like a node body
                // click (plain replaces the selection, Shift toggles this
                // pin's membership); a drag repositions instead (handled by
                // `PinUi::apply`).
                if response.clicked() {
                    let shift = ui.modifiers().shift;
                    click_intents(shift, scene, SelectionKey::Pin(g.out_port), out);
                }
            }
        }
    }
}

/// One pinned output's resolved geometry + breaker/hover state for this
/// frame — computed once, fed to both [`PinUi::draw_wire`] (the bezier,
/// pre-node-body) and [`PinUi::draw_widget`] (the port circle + card,
/// post-node-body) so a pin drawn in two passes still agrees on `broken`
/// and hover within the same frame. `None` when the wire's control hull
/// misses the visible rect entirely (culled).
struct PinGeometry {
    out_port: OutputPort,
    port_center: Vec2,
    top_left: Vec2,
    handles: CubicHandles,
    box_hover: bool,
    /// Whether the active breaker gesture crosses this pin's bezier or its
    /// widget rect — already folded into `probe.mark_broken_pin` here, so
    /// neither caller needs to repeat that.
    broken: bool,
}

#[allow(clippy::too_many_arguments)]
fn resolve_pin_geometry(
    ui: &Ui,
    geometry: &CanvasGeometry,
    probe: &mut BreakerProbe<'_>,
    visible: Option<Rect>,
    node_id: NodeId,
    port_idx: usize,
    output: &SceneOutput,
) -> Option<PinGeometry> {
    let port_ref = PortRef {
        node_id,
        kind: PortKind::Output,
        port_idx,
    };
    let port_center = geometry.ports.center(port_ref)?;
    let out_port = OutputPort::new(node_id, port_idx);
    // The wire's far end, and the preview widget's own top-left corner —
    // one and the same point, so the wire lands exactly on the port-circle
    // glyph peeking from under that corner.
    let top_left = output.pin_position;
    let handles = cubic_handles(port_center, top_left);
    if !wire_visible(visible, port_center, &handles, top_left) {
        return None;
    }
    let box_rect = pin_preview_rect(output);
    let broken = pin_targeted(probe, port_center, &handles, top_left, box_rect);
    if broken {
        probe.mark_broken_pin(out_port);
    }
    let box_hover = preview_hovered(ui, out_port);
    Some(PinGeometry {
        out_port,
        port_center,
        top_left,
        handles,
        box_hover,
        broken,
    })
}

/// True if the active breaker gesture crosses the pin's glyph — either the
/// connecting bezier or the preview widget's rect (matching how a node
/// body's breaker hit-test uses its rect rather than an exact shape).
fn pin_targeted(
    probe: &BreakerProbe<'_>,
    port_center: Vec2,
    handles: &CubicHandles,
    wire_end: Vec2,
    box_rect: Rect,
) -> bool {
    if probe.crosses_cubic(port_center, handles.p1, handles.p2, wire_end) {
        return true;
    }
    probe.crosses_rect(box_rect)
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
/// its current absolute position (the drag's start point) — or `None`.
/// Only a pinned output has a preview widget at all.
fn scan_widget_drag_start(ui: &Ui, scene: &Scene) -> Option<(OutputPort, Vec2)> {
    for n in &scene.nodes {
        for (i, output) in scene.outputs(n.outputs).iter().enumerate() {
            if !output.pinned {
                continue;
            }
            let port = OutputPort::new(n.id, i);
            if ui.response_for(pin_preview_wid(port)).drag_started() {
                return Some((port, output.pin_position));
            }
        }
    }
    None
}

/// `true` when the pointer is directly over `port`'s preview widget this
/// frame. Drives the bezier's endpoint-hover emphasis (like a data wire's)
/// and the port-circle glyph's own direct-hover tint (like a real port
/// circle's).
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

    fn box_rect_at(top_left: Vec2) -> Rect {
        Rect::new(top_left.x, top_left.y, PREVIEW_WIDTH, PREVIEW_HEIGHT)
    }

    #[test]
    fn pin_targeted_hits_the_preview_box_but_not_empty_space() {
        let port_center = Vec2::ZERO;
        let top_left = port_center + default_pin_offset();
        let handles = cubic_handles(port_center, top_left);
        let rect = box_rect_at(top_left);
        let box_center = top_left + Vec2::new(PREVIEW_WIDTH, PREVIEW_HEIGHT) * 0.5;

        let mut hit = BreakerState::start(box_center, PointerButton::Right);
        assert!(
            pin_targeted(&probe_for(&mut hit), port_center, &handles, top_left, rect),
            "a breaker sample landing dead-center in the preview widget must register"
        );

        let mut miss = BreakerState::start(Vec2::new(1000.0, 1000.0), PointerButton::Right);
        assert!(
            !pin_targeted(&probe_for(&mut miss), port_center, &handles, top_left, rect),
            "a breaker far from the glyph must not register"
        );
    }

    #[test]
    fn pin_targeted_hits_the_connecting_bezier() {
        // Synthetic, well-separated points (rather than `default_pin_offset`)
        // so the bezier's midpoint is unambiguously clear of the box rect —
        // this test exercises the bezier-crossing path, not the box-rect one.
        let port_center = Vec2::ZERO;
        let top_left = Vec2::new(300.0, 0.0);
        let handles = cubic_handles(port_center, top_left);
        let rect = box_rect_at(top_left);
        let mid = cubic_point(port_center, handles.p1, handles.p2, top_left, 0.53);

        let mut state = BreakerState::start(mid + Vec2::new(0.0, -80.0), PointerButton::Right);
        state.add_point(mid + Vec2::new(0.0, 80.0));
        assert!(pin_targeted(
            &probe_for(&mut state),
            port_center,
            &handles,
            top_left,
            rect
        ));
    }
}
